//! Minimal fail-closed SFTP v3 client for execution-scoped secret delivery.
//!
//! Submission graphs can be large and continue to use the existing text
//! transport. Publisher credentials use this binary protocol so their bytes
//! never appear in a remote command, process argument, or shell trace.

use crate::OrchestrationError;
use std::io::{Cursor, Read};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use zeroize::Zeroizing;

const FXP_INIT: u8 = 1;
const FXP_VERSION: u8 = 2;
const FXP_OPEN: u8 = 3;
const FXP_CLOSE: u8 = 4;
const FXP_WRITE: u8 = 6;
const FXP_LSTAT: u8 = 7;
const FXP_SETSTAT: u8 = 9;
const FXP_REMOVE: u8 = 13;
const FXP_MKDIR: u8 = 14;
const FXP_STATUS: u8 = 101;
const FXP_HANDLE: u8 = 102;
const FXP_ATTRS: u8 = 105;
const FXP_EXTENDED: u8 = 200;

const STATUS_OK: u32 = 0;
const STATUS_NO_SUCH_FILE: u32 = 2;
const ATTR_SIZE: u32 = 0x0000_0001;
const ATTR_UID_GID: u32 = 0x0000_0002;
const ATTR_PERMISSIONS: u32 = 0x0000_0004;
const ATTR_ACMODTIME: u32 = 0x0000_0008;
const ATTR_EXTENDED: u32 = 0x8000_0000;
const OPEN_WRITE: u32 = 0x0000_0002;
const OPEN_CREATE: u32 = 0x0000_0008;
const OPEN_TRUNCATE: u32 = 0x0000_0010;
const OPEN_EXCLUSIVE: u32 = 0x0000_0020;
const FILE_TYPE_MASK: u32 = 0o170000;
const FILE_TYPE_DIRECTORY: u32 = 0o040000;
const FILE_TYPE_REGULAR: u32 = 0o100000;
const MAX_PACKET_BYTES: usize = 16 * 1024 * 1024;
const WRITE_CHUNK_BYTES: usize = 64 * 1024;

pub(crate) struct SftpV3<S> {
    stream: S,
    next_request_id: u32,
}

impl<S> SftpV3<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    pub(crate) async fn connect(mut stream: S) -> Result<Self, OrchestrationError> {
        send_packet(&mut stream, &init_packet()).await?;
        let response = receive_packet(&mut stream).await?;
        let mut cursor = Cursor::new(response.as_slice());
        let packet_type = read_u8(&mut cursor)?;
        let version = read_u32(&mut cursor)?;
        if packet_type != FXP_VERSION || version != 3 {
            return Err(protocol_error("remote server did not negotiate SFTP v3"));
        }
        Ok(Self {
            stream,
            next_request_id: 1,
        })
    }

    pub(crate) async fn ensure_private_directory(
        &mut self,
        path: &str,
        mode: u32,
    ) -> Result<(), OrchestrationError> {
        match self.lstat_permissions(path).await? {
            Some(existing) => ensure_file_type(existing, FILE_TYPE_DIRECTORY, "directory")?,
            None => {
                let request_id = self.request_id();
                let mut packet = request_packet(FXP_MKDIR, request_id);
                put_string(&mut packet, path.as_bytes())?;
                put_permissions(&mut packet, mode);
                self.send_and_expect_ok(packet, request_id, "create private directory")
                    .await?;
            }
        }
        self.set_permissions(path, mode).await?;
        let observed = self.lstat_permissions(path).await?.ok_or_else(|| {
            protocol_error("private directory disappeared after permissions were set")
        })?;
        ensure_file_type(observed, FILE_TYPE_DIRECTORY, "directory")?;
        ensure_mode(observed, mode, "private directory")
    }

    pub(crate) async fn upload_private_file_atomic(
        &mut self,
        final_path: &str,
        temporary_path: &str,
        content: &[u8],
        mode: u32,
    ) -> Result<(), OrchestrationError> {
        let result = self
            .upload_private_file_atomic_inner(final_path, temporary_path, content, mode)
            .await;
        if result.is_err() {
            let _ = self.remove_file_if_present(temporary_path).await;
            // The server may have completed the atomic rename before its
            // response (or our final LSTAT response) was lost. Submission has
            // not started yet, so fail closed and remove both possible names.
            let _ = self.remove_file_if_present(final_path).await;
        }
        result
    }

    async fn upload_private_file_atomic_inner(
        &mut self,
        final_path: &str,
        temporary_path: &str,
        content: &[u8],
        mode: u32,
    ) -> Result<(), OrchestrationError> {
        let request_id = self.request_id();
        let mut packet = request_packet(FXP_OPEN, request_id);
        put_string(&mut packet, temporary_path.as_bytes())?;
        put_u32(
            &mut packet,
            OPEN_WRITE | OPEN_CREATE | OPEN_TRUNCATE | OPEN_EXCLUSIVE,
        );
        put_permissions(&mut packet, mode);
        self.send(packet).await?;
        let handle = self.expect_handle(request_id, "open private temporary file").await?;

        for (index, chunk) in content.chunks(WRITE_CHUNK_BYTES).enumerate() {
            let request_id = self.request_id();
            let mut packet = Zeroizing::new(request_packet(FXP_WRITE, request_id));
            put_string(&mut packet, &handle)?;
            put_u64(&mut packet, (index * WRITE_CHUNK_BYTES) as u64);
            put_string(&mut packet, chunk)?;
            self.send_bytes(packet.as_slice()).await?;
            let status = self
                .receive_status(request_id, "write private temporary file")
                .await?;
            if status != STATUS_OK {
                return Err(protocol_error(&format!(
                    "write private temporary file failed with SFTP status {status}"
                )));
            }
        }

        let request_id = self.request_id();
        let mut close = request_packet(FXP_CLOSE, request_id);
        put_string(&mut close, &handle)?;
        self.send_and_expect_ok(close, request_id, "close private temporary file")
            .await?;
        self.set_permissions(temporary_path, mode).await?;
        let observed = self
            .lstat_permissions(temporary_path)
            .await?
            .ok_or_else(|| protocol_error("private temporary file disappeared before rename"))?;
        ensure_file_type(observed, FILE_TYPE_REGULAR, "regular file")?;
        ensure_mode(observed, mode, "private temporary file")?;

        // SFTP v3's base RENAME operation does not guarantee overwrite or
        // atomicity. OpenSSH's posix-rename extension does; fail closed if the
        // server does not implement it.
        let request_id = self.request_id();
        let mut rename = request_packet(FXP_EXTENDED, request_id);
        put_string(&mut rename, b"posix-rename@openssh.com")?;
        put_string(&mut rename, temporary_path.as_bytes())?;
        put_string(&mut rename, final_path.as_bytes())?;
        self.send_and_expect_ok(rename, request_id, "atomically rename private file")
            .await?;

        let observed = self
            .lstat_permissions(final_path)
            .await?
            .ok_or_else(|| protocol_error("private file missing after atomic rename"))?;
        ensure_file_type(observed, FILE_TYPE_REGULAR, "regular file")?;
        ensure_mode(observed, mode, "private file")
    }

    pub(crate) async fn remove_file_if_present(
        &mut self,
        path: &str,
    ) -> Result<(), OrchestrationError> {
        let request_id = self.request_id();
        let mut packet = request_packet(FXP_REMOVE, request_id);
        put_string(&mut packet, path.as_bytes())?;
        self.send(packet).await?;
        match self.receive_status(request_id, "remove private file").await? {
            STATUS_OK | STATUS_NO_SUCH_FILE => Ok(()),
            status => Err(protocol_error(&format!(
                "remove private file failed with SFTP status {status}"
            ))),
        }
    }

    pub(crate) async fn shutdown(mut self) -> Result<(), OrchestrationError> {
        self.stream
            .shutdown()
            .await
            .map_err(|error| protocol_error(&format!("close SFTP stream: {error}")))
    }

    async fn set_permissions(
        &mut self,
        path: &str,
        mode: u32,
    ) -> Result<(), OrchestrationError> {
        let request_id = self.request_id();
        let mut packet = request_packet(FXP_SETSTAT, request_id);
        put_string(&mut packet, path.as_bytes())?;
        put_permissions(&mut packet, mode);
        self.send_and_expect_ok(packet, request_id, "set private path permissions")
            .await
    }

    async fn lstat_permissions(&mut self, path: &str) -> Result<Option<u32>, OrchestrationError> {
        let request_id = self.request_id();
        let mut packet = request_packet(FXP_LSTAT, request_id);
        put_string(&mut packet, path.as_bytes())?;
        self.send(packet).await?;
        let response = self.receive().await?;
        let mut cursor = Cursor::new(response.as_slice());
        let packet_type = read_u8(&mut cursor)?;
        let response_id = read_u32(&mut cursor)?;
        if response_id != request_id {
            return Err(protocol_error("SFTP response request ID mismatch"));
        }
        match packet_type {
            FXP_ATTRS => read_permissions(&mut cursor).map(Some),
            FXP_STATUS => {
                let status = read_u32(&mut cursor)?;
                if status == STATUS_NO_SUCH_FILE {
                    Ok(None)
                } else {
                    Err(protocol_error(&format!(
                        "inspect private path failed with SFTP status {status}"
                    )))
                }
            }
            _ => Err(protocol_error("unexpected SFTP path metadata response")),
        }
    }

    async fn expect_handle(
        &mut self,
        request_id: u32,
        operation: &str,
    ) -> Result<Vec<u8>, OrchestrationError> {
        let response = self.receive().await?;
        let mut cursor = Cursor::new(response.as_slice());
        let packet_type = read_u8(&mut cursor)?;
        let response_id = read_u32(&mut cursor)?;
        if response_id != request_id {
            return Err(protocol_error("SFTP response request ID mismatch"));
        }
        if packet_type == FXP_HANDLE {
            return read_string(&mut cursor);
        }
        if packet_type == FXP_STATUS {
            let status = read_u32(&mut cursor)?;
            return Err(protocol_error(&format!(
                "{operation} failed with SFTP status {status}"
            )));
        }
        Err(protocol_error("unexpected SFTP file-open response"))
    }

    async fn send_and_expect_ok(
        &mut self,
        packet: Vec<u8>,
        request_id: u32,
        operation: &str,
    ) -> Result<(), OrchestrationError> {
        self.send(packet).await?;
        let status = self.receive_status(request_id, operation).await?;
        if status == STATUS_OK {
            Ok(())
        } else {
            Err(protocol_error(&format!(
                "{operation} failed with SFTP status {status}"
            )))
        }
    }

    async fn receive_status(
        &mut self,
        request_id: u32,
        operation: &str,
    ) -> Result<u32, OrchestrationError> {
        let response = self.receive().await?;
        let mut cursor = Cursor::new(response.as_slice());
        let packet_type = read_u8(&mut cursor)?;
        let response_id = read_u32(&mut cursor)?;
        if packet_type != FXP_STATUS || response_id != request_id {
            return Err(protocol_error(&format!(
                "unexpected SFTP response while attempting to {operation}"
            )));
        }
        read_u32(&mut cursor)
    }

    async fn send(&mut self, packet: Vec<u8>) -> Result<(), OrchestrationError> {
        send_packet(&mut self.stream, &packet).await
    }

    async fn send_bytes(&mut self, packet: &[u8]) -> Result<(), OrchestrationError> {
        send_packet(&mut self.stream, packet).await
    }

    async fn receive(&mut self) -> Result<Vec<u8>, OrchestrationError> {
        receive_packet(&mut self.stream).await
    }

    fn request_id(&mut self) -> u32 {
        let value = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        value
    }
}

fn init_packet() -> Vec<u8> {
    let mut packet = vec![FXP_INIT];
    put_u32(&mut packet, 3);
    packet
}

fn request_packet(packet_type: u8, request_id: u32) -> Vec<u8> {
    let mut packet = vec![packet_type];
    put_u32(&mut packet, request_id);
    packet
}

fn put_permissions(packet: &mut Vec<u8>, mode: u32) {
    put_u32(packet, ATTR_PERMISSIONS);
    put_u32(packet, mode);
}

fn put_string(packet: &mut Vec<u8>, value: &[u8]) -> Result<(), OrchestrationError> {
    let length = u32::try_from(value.len())
        .map_err(|_| protocol_error("SFTP string exceeds the protocol size limit"))?;
    put_u32(packet, length);
    packet.extend_from_slice(value);
    Ok(())
}

fn put_u32(packet: &mut Vec<u8>, value: u32) {
    packet.extend_from_slice(&value.to_be_bytes());
}

fn put_u64(packet: &mut Vec<u8>, value: u64) {
    packet.extend_from_slice(&value.to_be_bytes());
}

async fn send_packet<S>(stream: &mut S, packet: &[u8]) -> Result<(), OrchestrationError>
where
    S: AsyncWrite + Unpin,
{
    let length = u32::try_from(packet.len())
        .map_err(|_| protocol_error("SFTP packet exceeds the protocol size limit"))?;
    stream
        .write_all(&length.to_be_bytes())
        .await
        .map_err(|error| protocol_error(&format!("write SFTP packet length: {error}")))?;
    stream
        .write_all(packet)
        .await
        .map_err(|error| protocol_error(&format!("write SFTP packet: {error}")))?;
    stream
        .flush()
        .await
        .map_err(|error| protocol_error(&format!("flush SFTP packet: {error}")))
}

async fn receive_packet<S>(stream: &mut S) -> Result<Vec<u8>, OrchestrationError>
where
    S: AsyncRead + Unpin,
{
    let mut length = [0_u8; 4];
    stream
        .read_exact(&mut length)
        .await
        .map_err(|error| protocol_error(&format!("read SFTP packet length: {error}")))?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_PACKET_BYTES {
        return Err(protocol_error("remote SFTP packet has an unsafe length"));
    }
    let mut packet = vec![0_u8; length];
    stream
        .read_exact(&mut packet)
        .await
        .map_err(|error| protocol_error(&format!("read SFTP packet: {error}")))?;
    Ok(packet)
}

fn read_permissions(cursor: &mut Cursor<&[u8]>) -> Result<u32, OrchestrationError> {
    let flags = read_u32(cursor)?;
    if flags & ATTR_SIZE != 0 {
        read_u64(cursor)?;
    }
    if flags & ATTR_UID_GID != 0 {
        read_u32(cursor)?;
        read_u32(cursor)?;
    }
    let permissions = if flags & ATTR_PERMISSIONS != 0 {
        Some(read_u32(cursor)?)
    } else {
        None
    };
    if flags & ATTR_ACMODTIME != 0 {
        read_u32(cursor)?;
        read_u32(cursor)?;
    }
    if flags & ATTR_EXTENDED != 0 {
        let count = read_u32(cursor)?;
        for _ in 0..count {
            read_string(cursor)?;
            read_string(cursor)?;
        }
    }
    permissions.ok_or_else(|| protocol_error("SFTP server omitted Unix permissions"))
}

fn read_u8(cursor: &mut Cursor<&[u8]>) -> Result<u8, OrchestrationError> {
    let mut bytes = [0_u8; 1];
    Read::read_exact(cursor, &mut bytes)
        .map_err(|_| protocol_error("truncated SFTP response"))?;
    Ok(bytes[0])
}

fn read_u32(cursor: &mut Cursor<&[u8]>) -> Result<u32, OrchestrationError> {
    let mut bytes = [0_u8; 4];
    Read::read_exact(cursor, &mut bytes)
        .map_err(|_| protocol_error("truncated SFTP response"))?;
    Ok(u32::from_be_bytes(bytes))
}

fn read_u64(cursor: &mut Cursor<&[u8]>) -> Result<u64, OrchestrationError> {
    let mut bytes = [0_u8; 8];
    Read::read_exact(cursor, &mut bytes)
        .map_err(|_| protocol_error("truncated SFTP response"))?;
    Ok(u64::from_be_bytes(bytes))
}

fn read_string(cursor: &mut Cursor<&[u8]>) -> Result<Vec<u8>, OrchestrationError> {
    let length = read_u32(cursor)? as usize;
    if length > MAX_PACKET_BYTES {
        return Err(protocol_error("SFTP string has an unsafe length"));
    }
    let mut bytes = vec![0_u8; length];
    Read::read_exact(cursor, &mut bytes)
        .map_err(|_| protocol_error("truncated SFTP response"))?;
    Ok(bytes)
}

fn ensure_file_type(
    permissions: u32,
    expected: u32,
    label: &str,
) -> Result<(), OrchestrationError> {
    if permissions & FILE_TYPE_MASK == expected {
        Ok(())
    } else {
        Err(protocol_error(&format!(
            "private path is not a {label}; refusing to follow it"
        )))
    }
}

fn ensure_mode(permissions: u32, expected: u32, label: &str) -> Result<(), OrchestrationError> {
    if permissions & 0o7777 == expected {
        Ok(())
    } else {
        Err(protocol_error(&format!(
            "{label} permissions are not {:04o}",
            expected
        )))
    }
}

fn protocol_error(message: &str) -> OrchestrationError {
    OrchestrationError::Backend(format!("SFTP secret delivery: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn read_client_packet(stream: &mut tokio::io::DuplexStream) -> Vec<u8> {
        receive_packet(stream).await.unwrap()
    }

    async fn send_server_packet(stream: &mut tokio::io::DuplexStream, packet: Vec<u8>) {
        send_packet(stream, &packet).await.unwrap();
    }

    fn version_packet() -> Vec<u8> {
        let mut packet = vec![FXP_VERSION];
        put_u32(&mut packet, 3);
        packet
    }

    fn status_packet(request_id: u32, status: u32) -> Vec<u8> {
        let mut packet = request_packet(FXP_STATUS, request_id);
        put_u32(&mut packet, status);
        put_string(&mut packet, b"").unwrap();
        put_string(&mut packet, b"").unwrap();
        packet
    }

    fn attrs_packet(request_id: u32, permissions: u32) -> Vec<u8> {
        let mut packet = request_packet(FXP_ATTRS, request_id);
        put_permissions(&mut packet, permissions);
        packet
    }

    fn handle_packet(request_id: u32, handle: &[u8]) -> Vec<u8> {
        let mut packet = request_packet(FXP_HANDLE, request_id);
        put_string(&mut packet, handle).unwrap();
        packet
    }

    fn request_cursor(packet: &[u8], expected_type: u8, expected_id: u32) -> Cursor<&[u8]> {
        let mut cursor = Cursor::new(packet);
        assert_eq!(read_u8(&mut cursor).unwrap(), expected_type);
        assert_eq!(read_u32(&mut cursor).unwrap(), expected_id);
        cursor
    }

    async fn negotiate_server(stream: &mut tokio::io::DuplexStream) {
        let init = read_client_packet(stream).await;
        let mut cursor = Cursor::new(init.as_slice());
        assert_eq!(read_u8(&mut cursor).unwrap(), FXP_INIT);
        assert_eq!(read_u32(&mut cursor).unwrap(), 3);
        send_server_packet(stream, version_packet()).await;
    }

    #[test]
    fn write_packet_preserves_arbitrary_secret_bytes() {
        let content = b"opaque\0token\nwith\xffbytes";
        let mut packet = request_packet(FXP_WRITE, 7);
        put_string(&mut packet, b"handle").unwrap();
        put_u64(&mut packet, 0);
        put_string(&mut packet, content).unwrap();
        assert!(packet.windows(content.len()).any(|window| window == content));
    }

    #[test]
    fn attributes_require_type_and_exact_private_mode() {
        assert!(ensure_file_type(0o040700, FILE_TYPE_DIRECTORY, "directory").is_ok());
        assert!(ensure_mode(0o040700, 0o700, "directory").is_ok());
        assert!(ensure_file_type(0o120700, FILE_TYPE_DIRECTORY, "directory").is_err());
        assert!(ensure_mode(0o100640, 0o600, "file").is_err());
    }

    #[tokio::test]
    async fn scripted_sftp_upload_is_binary_safe_private_and_atomic() {
        let (client, mut server) = tokio::io::duplex(1024 * 1024);
        let secret = b"opaque\0token\nwith\xffbytes".to_vec();
        let expected_secret = secret.clone();
        let server_task = tokio::spawn(async move {
            negotiate_server(&mut server).await;

            let packet = read_client_packet(&mut server).await;
            let mut cursor = request_cursor(&packet, FXP_LSTAT, 1);
            assert_eq!(
                read_string(&mut cursor).unwrap(),
                b"/dlg/session/.beampipe-secrets"
            );
            send_server_packet(&mut server, status_packet(1, STATUS_NO_SUCH_FILE)).await;

            let packet = read_client_packet(&mut server).await;
            let mut cursor = request_cursor(&packet, FXP_MKDIR, 2);
            assert_eq!(
                read_string(&mut cursor).unwrap(),
                b"/dlg/session/.beampipe-secrets"
            );
            assert_eq!(read_permissions(&mut cursor).unwrap(), 0o700);
            send_server_packet(&mut server, status_packet(2, STATUS_OK)).await;

            let packet = read_client_packet(&mut server).await;
            let mut cursor = request_cursor(&packet, FXP_SETSTAT, 3);
            assert_eq!(
                read_string(&mut cursor).unwrap(),
                b"/dlg/session/.beampipe-secrets"
            );
            assert_eq!(read_permissions(&mut cursor).unwrap(), 0o700);
            send_server_packet(&mut server, status_packet(3, STATUS_OK)).await;

            let packet = read_client_packet(&mut server).await;
            let mut cursor = request_cursor(&packet, FXP_LSTAT, 4);
            assert_eq!(
                read_string(&mut cursor).unwrap(),
                b"/dlg/session/.beampipe-secrets"
            );
            send_server_packet(&mut server, attrs_packet(4, 0o040700)).await;

            let packet = read_client_packet(&mut server).await;
            let mut cursor = request_cursor(&packet, FXP_OPEN, 5);
            assert_eq!(
                read_string(&mut cursor).unwrap(),
                b"/dlg/session/.beampipe-secrets/publisher.token.tmp-test"
            );
            assert_eq!(
                read_u32(&mut cursor).unwrap(),
                OPEN_WRITE | OPEN_CREATE | OPEN_TRUNCATE | OPEN_EXCLUSIVE
            );
            assert_eq!(read_permissions(&mut cursor).unwrap(), 0o600);
            send_server_packet(&mut server, handle_packet(5, b"private-handle")).await;

            let packet = read_client_packet(&mut server).await;
            let mut cursor = request_cursor(&packet, FXP_WRITE, 6);
            assert_eq!(read_string(&mut cursor).unwrap(), b"private-handle");
            assert_eq!(read_u64(&mut cursor).unwrap(), 0);
            assert_eq!(read_string(&mut cursor).unwrap(), expected_secret);
            send_server_packet(&mut server, status_packet(6, STATUS_OK)).await;

            let packet = read_client_packet(&mut server).await;
            let mut cursor = request_cursor(&packet, FXP_CLOSE, 7);
            assert_eq!(read_string(&mut cursor).unwrap(), b"private-handle");
            send_server_packet(&mut server, status_packet(7, STATUS_OK)).await;

            let packet = read_client_packet(&mut server).await;
            let mut cursor = request_cursor(&packet, FXP_SETSTAT, 8);
            assert_eq!(
                read_string(&mut cursor).unwrap(),
                b"/dlg/session/.beampipe-secrets/publisher.token.tmp-test"
            );
            assert_eq!(read_permissions(&mut cursor).unwrap(), 0o600);
            send_server_packet(&mut server, status_packet(8, STATUS_OK)).await;

            let packet = read_client_packet(&mut server).await;
            let mut cursor = request_cursor(&packet, FXP_LSTAT, 9);
            assert_eq!(
                read_string(&mut cursor).unwrap(),
                b"/dlg/session/.beampipe-secrets/publisher.token.tmp-test"
            );
            send_server_packet(&mut server, attrs_packet(9, 0o100600)).await;

            let packet = read_client_packet(&mut server).await;
            let mut cursor = request_cursor(&packet, FXP_EXTENDED, 10);
            assert_eq!(
                read_string(&mut cursor).unwrap(),
                b"posix-rename@openssh.com"
            );
            assert_eq!(
                read_string(&mut cursor).unwrap(),
                b"/dlg/session/.beampipe-secrets/publisher.token.tmp-test"
            );
            assert_eq!(
                read_string(&mut cursor).unwrap(),
                b"/dlg/session/.beampipe-secrets/publisher.token"
            );
            send_server_packet(&mut server, status_packet(10, STATUS_OK)).await;

            let packet = read_client_packet(&mut server).await;
            let mut cursor = request_cursor(&packet, FXP_LSTAT, 11);
            assert_eq!(
                read_string(&mut cursor).unwrap(),
                b"/dlg/session/.beampipe-secrets/publisher.token"
            );
            send_server_packet(&mut server, attrs_packet(11, 0o100600)).await;
        });

        let mut sftp = SftpV3::connect(client).await.unwrap();
        sftp.ensure_private_directory("/dlg/session/.beampipe-secrets", 0o700)
            .await
            .unwrap();
        sftp.upload_private_file_atomic(
            "/dlg/session/.beampipe-secrets/publisher.token",
            "/dlg/session/.beampipe-secrets/publisher.token.tmp-test",
            &secret,
            0o600,
        )
        .await
        .unwrap();
        sftp.shutdown().await.unwrap();
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn failed_secret_write_is_cleaned_without_echoing_secret() {
        let (client, mut server) = tokio::io::duplex(1024 * 1024);
        let secret = b"never-print-this-secret\0\xff".to_vec();
        let expected_secret = secret.clone();
        let server_task = tokio::spawn(async move {
            negotiate_server(&mut server).await;

            let packet = read_client_packet(&mut server).await;
            request_cursor(&packet, FXP_LSTAT, 1);
            send_server_packet(&mut server, attrs_packet(1, 0o040700)).await;
            let packet = read_client_packet(&mut server).await;
            request_cursor(&packet, FXP_SETSTAT, 2);
            send_server_packet(&mut server, status_packet(2, STATUS_OK)).await;
            let packet = read_client_packet(&mut server).await;
            request_cursor(&packet, FXP_LSTAT, 3);
            send_server_packet(&mut server, attrs_packet(3, 0o040700)).await;

            let packet = read_client_packet(&mut server).await;
            request_cursor(&packet, FXP_OPEN, 4);
            send_server_packet(&mut server, handle_packet(4, b"handle")).await;
            let packet = read_client_packet(&mut server).await;
            let mut cursor = request_cursor(&packet, FXP_WRITE, 5);
            assert_eq!(read_string(&mut cursor).unwrap(), b"handle");
            assert_eq!(read_u64(&mut cursor).unwrap(), 0);
            assert_eq!(read_string(&mut cursor).unwrap(), expected_secret);
            send_server_packet(&mut server, status_packet(5, 4)).await;

            let packet = read_client_packet(&mut server).await;
            let mut cursor = request_cursor(&packet, FXP_REMOVE, 6);
            assert_eq!(
                read_string(&mut cursor).unwrap(),
                b"/dlg/session/.beampipe-secrets/publisher.token.tmp-error"
            );
            send_server_packet(&mut server, status_packet(6, STATUS_OK)).await;

            let packet = read_client_packet(&mut server).await;
            let mut cursor = request_cursor(&packet, FXP_REMOVE, 7);
            assert_eq!(
                read_string(&mut cursor).unwrap(),
                b"/dlg/session/.beampipe-secrets/publisher.token"
            );
            send_server_packet(&mut server, status_packet(7, STATUS_NO_SUCH_FILE)).await;
        });

        let mut sftp = SftpV3::connect(client).await.unwrap();
        sftp.ensure_private_directory("/dlg/session/.beampipe-secrets", 0o700)
            .await
            .unwrap();
        let error = sftp
            .upload_private_file_atomic(
                "/dlg/session/.beampipe-secrets/publisher.token",
                "/dlg/session/.beampipe-secrets/publisher.token.tmp-error",
                &secret,
                0o600,
            )
            .await
            .unwrap_err();
        let rendered = format!("{error:?} {error}");
        assert!(!rendered
            .as_bytes()
            .windows(secret.len())
            .any(|window| window == secret.as_slice()));
        assert!(rendered.contains("status 4"));
        sftp.shutdown().await.unwrap();
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn post_rename_verification_failure_removes_both_secret_names() {
        let (client, mut server) = tokio::io::duplex(1024 * 1024);
        let secret = b"never-leave-this-token-after-rename-0123456789".to_vec();
        let expected_secret = secret.clone();
        let server_task = tokio::spawn(async move {
            negotiate_server(&mut server).await;

            let packet = read_client_packet(&mut server).await;
            request_cursor(&packet, FXP_LSTAT, 1);
            send_server_packet(&mut server, attrs_packet(1, 0o040700)).await;
            let packet = read_client_packet(&mut server).await;
            request_cursor(&packet, FXP_SETSTAT, 2);
            send_server_packet(&mut server, status_packet(2, STATUS_OK)).await;
            let packet = read_client_packet(&mut server).await;
            request_cursor(&packet, FXP_LSTAT, 3);
            send_server_packet(&mut server, attrs_packet(3, 0o040700)).await;

            let packet = read_client_packet(&mut server).await;
            request_cursor(&packet, FXP_OPEN, 4);
            send_server_packet(&mut server, handle_packet(4, b"handle")).await;
            let packet = read_client_packet(&mut server).await;
            let mut cursor = request_cursor(&packet, FXP_WRITE, 5);
            assert_eq!(read_string(&mut cursor).unwrap(), b"handle");
            assert_eq!(read_u64(&mut cursor).unwrap(), 0);
            assert_eq!(read_string(&mut cursor).unwrap(), expected_secret);
            send_server_packet(&mut server, status_packet(5, STATUS_OK)).await;
            let packet = read_client_packet(&mut server).await;
            request_cursor(&packet, FXP_CLOSE, 6);
            send_server_packet(&mut server, status_packet(6, STATUS_OK)).await;
            let packet = read_client_packet(&mut server).await;
            request_cursor(&packet, FXP_SETSTAT, 7);
            send_server_packet(&mut server, status_packet(7, STATUS_OK)).await;
            let packet = read_client_packet(&mut server).await;
            request_cursor(&packet, FXP_LSTAT, 8);
            send_server_packet(&mut server, attrs_packet(8, 0o100600)).await;

            let packet = read_client_packet(&mut server).await;
            let mut cursor = request_cursor(&packet, FXP_EXTENDED, 9);
            assert_eq!(
                read_string(&mut cursor).unwrap(),
                b"posix-rename@openssh.com"
            );
            send_server_packet(&mut server, status_packet(9, STATUS_OK)).await;
            let packet = read_client_packet(&mut server).await;
            request_cursor(&packet, FXP_LSTAT, 10);
            send_server_packet(&mut server, status_packet(10, 4)).await;

            for (request_id, expected_path) in [
                (
                    11,
                    b"/dlg/session/.beampipe-secrets/publisher.token.tmp-rename".as_slice(),
                ),
                (
                    12,
                    b"/dlg/session/.beampipe-secrets/publisher.token".as_slice(),
                ),
            ] {
                let packet = read_client_packet(&mut server).await;
                let mut cursor = request_cursor(&packet, FXP_REMOVE, request_id);
                assert_eq!(read_string(&mut cursor).unwrap(), expected_path);
                send_server_packet(&mut server, status_packet(request_id, STATUS_OK)).await;
            }
        });

        let mut sftp = SftpV3::connect(client).await.unwrap();
        sftp.ensure_private_directory("/dlg/session/.beampipe-secrets", 0o700)
            .await
            .unwrap();
        let error = sftp
            .upload_private_file_atomic(
                "/dlg/session/.beampipe-secrets/publisher.token",
                "/dlg/session/.beampipe-secrets/publisher.token.tmp-rename",
                &secret,
                0o600,
            )
            .await
            .unwrap_err();
        let rendered = format!("{error:?} {error}");
        assert!(rendered.contains("status 4"));
        assert!(!rendered.contains("never-leave-this-token"));
        sftp.shutdown().await.unwrap();
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn unexpected_request_ids_and_oversized_packets_fail_closed() {
        let (client, mut server) = tokio::io::duplex(4096);
        let server_task = tokio::spawn(async move {
            negotiate_server(&mut server).await;
            let packet = read_client_packet(&mut server).await;
            request_cursor(&packet, FXP_REMOVE, 1);
            send_server_packet(&mut server, status_packet(999, STATUS_OK)).await;
        });
        let mut sftp = SftpV3::connect(client).await.unwrap();
        let mismatch = sftp.remove_file_if_present("/private/token").await.unwrap_err();
        assert!(mismatch.to_string().contains("unexpected SFTP response"));
        sftp.shutdown().await.unwrap();
        server_task.await.unwrap();

        let (client, mut server) = tokio::io::duplex(4096);
        let server_task = tokio::spawn(async move {
            let init = read_client_packet(&mut server).await;
            assert_eq!(init[0], FXP_INIT);
            server
                .write_all(&((MAX_PACKET_BYTES as u32) + 1).to_be_bytes())
                .await
                .unwrap();
            server.flush().await.unwrap();
        });
        let oversized = SftpV3::connect(client).await.err().unwrap();
        assert!(oversized.to_string().contains("unsafe length"));
        server_task.await.unwrap();
    }
}
