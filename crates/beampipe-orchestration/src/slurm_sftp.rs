//! Fail-closed SFTP policy for execution-scoped secret delivery.
//!
//! `openssh-sftp-client` owns SFTP framing, request correlation, parsing, and
//! OpenSSH extension handling. This module keeps only Beampipe's policy:
//! private path types and modes, exclusive temporary creation, durable writes,
//! atomic rename, post-write verification, cleanup, and redacted errors.

use crate::OrchestrationError;
use openssh_sftp_client::{
    error::{Error as SftpError, SftpErrorKind},
    metadata::{MetaData, Permissions},
    Sftp, SftpOptions,
};
use std::time::Duration;
use tokio::io::{self, AsyncRead, AsyncWrite};

const DIRECTORY_MODE: u16 = 0o700;
const FILE_MODE: u16 = 0o600;

pub(crate) struct PrivateSftp {
    client: Sftp,
}

impl PrivateSftp {
    pub(crate) async fn connect<S>(stream: S) -> Result<Self, OrchestrationError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (stdout, stdin) = io::split(stream);
        Self::connect_parts(stdin, stdout).await
    }

    async fn connect_parts<W, R>(stdin: W, stdout: R) -> Result<Self, OrchestrationError>
    where
        W: AsyncWrite + Send + 'static,
        R: AsyncRead + Send + 'static,
    {
        let options = SftpOptions::new().flush_interval(Duration::ZERO);
        let client = Sftp::new(stdin, stdout, options)
            .await
            .map_err(|error| sftp_error("start SFTP session", &error))?;

        if !client.support_posix_rename() {
            let _ = client.close().await;
            return Err(protocol_error(
                "server does not support atomic posix-rename@openssh.com",
            ));
        }
        if !client.support_fsync() {
            let _ = client.close().await;
            return Err(protocol_error(
                "server does not support durable fsync@openssh.com",
            ));
        }

        Ok(Self { client })
    }

    pub(crate) async fn ensure_private_directory(
        &mut self,
        path: &str,
        mode: u32,
    ) -> Result<(), OrchestrationError> {
        require_mode(mode, DIRECTORY_MODE, "private directory")?;
        let permissions = Permissions::from(DIRECTORY_MODE);
        let mut fs = self.client.fs();

        match fs.symlink_metadata(path).await {
            Ok(metadata) => ensure_directory(&metadata)?,
            Err(error) if is_missing(&error) => {
                fs.dir_builder()
                    .permissions(permissions)
                    .create(path)
                    .await
                    .map_err(|error| sftp_error("create private directory", &error))?;
            }
            Err(error) => return Err(sftp_error("inspect private directory", &error)),
        }

        fs.set_permissions(path, permissions)
            .await
            .map_err(|error| sftp_error("set private directory permissions", &error))?;
        let metadata = fs
            .symlink_metadata(path)
            .await
            .map_err(|error| sftp_error("verify private directory", &error))?;
        ensure_directory(&metadata)?;
        ensure_mode(&metadata, DIRECTORY_MODE, "private directory")
    }

    pub(crate) async fn upload_private_file_atomic(
        &mut self,
        final_path: &str,
        temporary_path: &str,
        content: &[u8],
        mode: u32,
    ) -> Result<(), OrchestrationError> {
        require_mode(mode, FILE_MODE, "private file")?;
        let result = self
            .upload_private_file_atomic_inner(final_path, temporary_path, content)
            .await;
        if result.is_err() {
            let _ = self.remove_file_if_present(temporary_path).await;
            // A response can be lost after the server commits the rename.
            // Submission has not started, so remove both possible names.
            let _ = self.remove_file_if_present(final_path).await;
        }
        result
    }

    async fn upload_private_file_atomic_inner(
        &mut self,
        final_path: &str,
        temporary_path: &str,
        content: &[u8],
    ) -> Result<(), OrchestrationError> {
        let permissions = Permissions::from(FILE_MODE);
        let mut file = self
            .client
            .options()
            .write(true)
            .create_new(true)
            .open(temporary_path)
            .await
            .map_err(|error| sftp_error("create private temporary file", &error))?;

        file.set_permissions(permissions)
            .await
            .map_err(|error| sftp_error("set private temporary file permissions", &error))?;
        file.write_all(content)
            .await
            .map_err(|error| sftp_error("write private temporary file", &error))?;
        file.sync_all()
            .await
            .map_err(|error| sftp_error("sync private temporary file", &error))?;
        file.close()
            .await
            .map_err(|error| sftp_error("close private temporary file", &error))?;

        let mut fs = self.client.fs();
        let temporary_metadata = fs
            .symlink_metadata(temporary_path)
            .await
            .map_err(|error| sftp_error("verify private temporary file", &error))?;
        ensure_regular_file(&temporary_metadata)?;
        ensure_mode(&temporary_metadata, FILE_MODE, "private temporary file")?;
        ensure_size(&temporary_metadata, content.len(), "private temporary file")?;

        // connect_parts already required POSIX rename support. The library
        // therefore selects posix-rename@openssh.com rather than base RENAME.
        fs.rename(temporary_path, final_path)
            .await
            .map_err(|error| sftp_error("atomically rename private file", &error))?;

        let final_metadata = fs
            .symlink_metadata(final_path)
            .await
            .map_err(|error| sftp_error("verify private file", &error))?;
        ensure_regular_file(&final_metadata)?;
        ensure_mode(&final_metadata, FILE_MODE, "private file")?;
        ensure_size(&final_metadata, content.len(), "private file")
    }

    pub(crate) async fn remove_file_if_present(
        &mut self,
        path: &str,
    ) -> Result<(), OrchestrationError> {
        let mut fs = self.client.fs();
        match fs.remove_file(path).await {
            Ok(()) => Ok(()),
            Err(error) if is_missing(&error) => Ok(()),
            Err(error) => Err(sftp_error("remove private file", &error)),
        }
    }

    pub(crate) async fn shutdown(self) -> Result<(), OrchestrationError> {
        self.client
            .close()
            .await
            .map_err(|error| sftp_error("close SFTP session", &error))
    }
}

fn ensure_directory(metadata: &MetaData) -> Result<(), OrchestrationError> {
    match metadata.file_type() {
        Some(file_type) if file_type.is_dir() => Ok(()),
        _ => Err(protocol_error(
            "private path is not a directory; refusing to follow it",
        )),
    }
}

fn ensure_regular_file(metadata: &MetaData) -> Result<(), OrchestrationError> {
    match metadata.file_type() {
        Some(file_type) if file_type.is_file() => Ok(()),
        _ => Err(protocol_error(
            "private path is not a regular file; refusing to follow it",
        )),
    }
}

fn ensure_mode(metadata: &MetaData, expected: u16, label: &str) -> Result<(), OrchestrationError> {
    let observed = metadata
        .permissions()
        .ok_or_else(|| protocol_error("SFTP server omitted Unix permissions"))?
        .as_raw()
        .bits()
        & 0o7777;
    if observed == u32::from(expected) {
        Ok(())
    } else {
        Err(protocol_error(&format!(
            "{label} permissions are not {expected:04o}",
        )))
    }
}

fn ensure_size(
    metadata: &MetaData,
    expected: usize,
    label: &str,
) -> Result<(), OrchestrationError> {
    let expected = u64::try_from(expected)
        .map_err(|_| protocol_error("private file exceeds the supported size"))?;
    match metadata.len() {
        Some(observed) if observed == expected => Ok(()),
        Some(_) => Err(protocol_error(&format!(
            "{label} size does not match the uploaded credential",
        ))),
        None => Err(protocol_error("SFTP server omitted private file size")),
    }
}

fn require_mode(actual: u32, expected: u16, label: &str) -> Result<(), OrchestrationError> {
    if actual == u32::from(expected) {
        Ok(())
    } else {
        Err(protocol_error(&format!(
            "{label} mode must be {expected:04o}",
        )))
    }
}

fn is_missing(error: &SftpError) -> bool {
    matches!(error, SftpError::SftpError(SftpErrorKind::NoSuchFile, _))
}

fn sftp_error(operation: &str, error: &SftpError) -> OrchestrationError {
    // Never include a server-controlled SFTP error message. A remote peer has
    // seen the credential bytes and could reflect them into logs deliberately.
    let category = match error {
        SftpError::SftpError(kind, _) => match kind {
            SftpErrorKind::NoSuchFile => "remote path was not found",
            SftpErrorKind::PermDenied => "remote permission was denied",
            SftpErrorKind::Failure => "remote operation failed",
            SftpErrorKind::BadMessage => "remote rejected the request",
            SftpErrorKind::OpUnsupported => "remote operation is unsupported",
            SftpErrorKind::Unknown => "remote returned an unknown status",
            _ => "remote returned an unrecognized status",
        },
        SftpError::UnsupportedSftpProtocol { .. } => "server does not support SFTP v3",
        SftpError::SftpServerHelloMsgTooLong { .. } => "server hello exceeded the safe limit",
        SftpError::UnsupportedExtension(_) => "required server extension is unavailable",
        SftpError::InvalidResponseId { .. } => "server returned an invalid response ID",
        SftpError::InvalidResponse(_) => "server returned an invalid response",
        SftpError::BufferTooLong(_) | SftpError::HandleTooLong => {
            "protocol value exceeded the safe limit"
        }
        SftpError::IOError(_) => "SFTP transport failed",
        SftpError::AwaitableError(_) => "SFTP response wait failed",
        SftpError::BackgroundTaskFailure(_) | SftpError::TaskJoinError(_) => {
            "SFTP background task failed"
        }
        SftpError::FormatError(_) => "SFTP message encoding failed",
        SftpError::RecursiveErrors(_) | SftpError::RecursiveErrors3(_) => {
            "SFTP operation and cleanup failed"
        }
        _ => "SFTP operation failed",
    };
    protocol_error(&format!("{operation}: {category}"))
}

fn protocol_error(message: &str) -> OrchestrationError {
    OrchestrationError::Backend(format!("SFTP secret delivery: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn requested_modes_are_fixed_and_fail_closed() {
        assert!(require_mode(0o700, DIRECTORY_MODE, "directory").is_ok());
        assert!(require_mode(0o600, FILE_MODE, "file").is_ok());
        assert!(require_mode(0o750, DIRECTORY_MODE, "directory").is_err());
        assert!(require_mode(0o640, FILE_MODE, "file").is_err());
    }

    #[test]
    fn server_controlled_errors_are_redacted() {
        static SECRET: &str = "do-not-log-this-token";
        let error = SftpError::InvalidResponse(&SECRET);
        let rendered = sftp_error("write private file", &error).to_string();
        assert!(!rendered.contains(SECRET));
        assert!(rendered.contains("invalid response"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn openssh_server_upload_is_binary_safe_private_and_atomic() {
        let server = ["/usr/lib/openssh/sftp-server", "/usr/lib/ssh/sftp-server"]
            .into_iter()
            .find(|path| std::path::Path::new(path).is_file());
        let Some(server) = server else {
            eprintln!("skipping local SFTP integration: sftp-server is unavailable");
            return;
        };

        let root = tempfile::tempdir().unwrap();
        let secret_directory = root.path().join("private");
        let final_path = secret_directory.join("publisher.token");
        let temporary_path = secret_directory.join("publisher.token.tmp-test");
        let link_path = root.path().join("private-link");
        let secret = b"opaque\0token\nwith\xffbytes";

        let mut child = tokio::process::Command::new(server)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut sftp = PrivateSftp::connect_parts(stdin, stdout).await.unwrap();

        sftp.ensure_private_directory(secret_directory.to_str().unwrap(), 0o700)
            .await
            .unwrap();
        sftp.upload_private_file_atomic(
            final_path.to_str().unwrap(),
            temporary_path.to_str().unwrap(),
            secret,
            0o600,
        )
        .await
        .unwrap();

        assert_eq!(std::fs::read(&final_path).unwrap(), secret);
        assert!(!temporary_path.exists());
        assert_eq!(
            std::fs::metadata(&secret_directory)
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o700,
        );
        assert_eq!(
            std::fs::metadata(&final_path).unwrap().permissions().mode() & 0o7777,
            0o600,
        );

        std::os::unix::fs::symlink(&secret_directory, &link_path).unwrap();
        let error = sftp
            .ensure_private_directory(link_path.to_str().unwrap(), 0o700)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("refusing to follow"));

        sftp.shutdown().await.unwrap();
        let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(status.success());
    }
}
