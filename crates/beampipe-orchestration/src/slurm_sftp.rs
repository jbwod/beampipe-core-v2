//! Fail-closed SFTP policy for Slurm submission artifacts and output receipts.
//!
//! `openssh-sftp-client` owns SFTP framing, request correlation, parsing, and
//! OpenSSH extension handling. This module keeps only Beampipe's policy:
//! accepted modes, exclusive temporary creation, durable writes, atomic rename,
//! post-write verification, cleanup, and redacted errors.

use crate::OrchestrationError;
use openssh_sftp_client::{
    error::{Error as SftpError, SftpErrorKind},
    file::File,
    metadata::{MetaData, Permissions},
    Sftp, SftpOptions,
};
use std::time::Duration;
use tokio::io::{self, AsyncRead, AsyncSeekExt, AsyncWrite};

/// Generated submission artifacts can contain short-lived signed URLs, so the
/// default remains owner-only even though the generic uploader also supports
/// read-only group/world access for non-secret artifacts.
pub(crate) const SUBMISSION_ARTIFACT_MODE: u32 = 0o600;
pub(crate) const OUTPUT_HANDOFF_DIRECTORY_MODE: u32 = 0o700;
pub(crate) const OUTPUT_INVENTORY_FILE_MODE: u32 = 0o600;

pub(crate) struct RemoteSftp {
    client: Sftp,
}

impl RemoteSftp {
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

    /// Upload a non-secret artifact through an exclusive same-directory
    /// temporary file, durably sync it, and atomically replace the final name.
    pub(crate) async fn upload_file_atomic(
        &mut self,
        final_path: &str,
        temporary_path: &str,
        content: &[u8],
        mode: u32,
    ) -> Result<(), OrchestrationError> {
        let mode = artifact_mode(mode)?;
        self.upload_file_atomic_with_mode(
            final_path,
            temporary_path,
            content,
            mode,
            "submission artifact",
        )
        .await
    }

    async fn upload_file_atomic_with_mode(
        &mut self,
        final_path: &str,
        temporary_path: &str,
        content: &[u8],
        mode: u16,
        label: &str,
    ) -> Result<(), OrchestrationError> {
        let result = self
            .upload_file_atomic_inner(final_path, temporary_path, content, mode, label)
            .await;
        if result.is_err() {
            let _ = self.remove_file_if_present(temporary_path).await;
            // A response can be lost after the server commits the rename.
            // Submission has not started, so remove both possible names.
            let _ = self.remove_file_if_present(final_path).await;
        }
        result
    }

    async fn upload_file_atomic_inner(
        &mut self,
        final_path: &str,
        temporary_path: &str,
        content: &[u8],
        mode: u16,
        label: &str,
    ) -> Result<(), OrchestrationError> {
        let permissions = Permissions::from(mode);
        let temporary_label = format!("temporary {label}");
        let mut file = self
            .client
            .options()
            .write(true)
            .create_new(true)
            .open(temporary_path)
            .await
            .map_err(|error| sftp_error(&format!("create {temporary_label}"), &error))?;

        file.set_permissions(permissions)
            .await
            .map_err(|error| sftp_error(&format!("set {temporary_label} permissions"), &error))?;
        file.write_all(content)
            .await
            .map_err(|error| sftp_error(&format!("write {temporary_label}"), &error))?;
        file.sync_all()
            .await
            .map_err(|error| sftp_error(&format!("sync {temporary_label}"), &error))?;
        file.close()
            .await
            .map_err(|error| sftp_error(&format!("close {temporary_label}"), &error))?;

        let mut fs = self.client.fs();
        let temporary_metadata = fs
            .symlink_metadata(temporary_path)
            .await
            .map_err(|error| sftp_error(&format!("verify {temporary_label}"), &error))?;
        ensure_regular_file(&temporary_metadata, &temporary_label)?;
        ensure_mode(&temporary_metadata, mode, &temporary_label)?;
        ensure_size(&temporary_metadata, content.len(), &temporary_label)?;

        // connect_parts already required POSIX rename support. The library
        // therefore selects posix-rename@openssh.com rather than base RENAME.
        fs.rename(temporary_path, final_path)
            .await
            .map_err(|error| sftp_error(&format!("atomically rename {label}"), &error))?;

        let final_metadata = fs
            .symlink_metadata(final_path)
            .await
            .map_err(|error| sftp_error(&format!("verify {label}"), &error))?;
        ensure_regular_file(&final_metadata, label)?;
        ensure_mode(&final_metadata, mode, label)?;
        ensure_size(&final_metadata, content.len(), label)
    }

    pub(crate) async fn remove_file_if_present(
        &mut self,
        path: &str,
    ) -> Result<(), OrchestrationError> {
        let mut fs = self.client.fs();
        match fs.remove_file(path).await {
            Ok(()) => Ok(()),
            Err(error) if is_missing(&error) => Ok(()),
            Err(error) => Err(sftp_error("remove remote file", &error)),
        }
    }

    /// Read a publisher handoff without following a final symlink and without
    /// trusting remote size metadata as an allocation bound.
    pub(crate) async fn read_output_inventory(
        &mut self,
        control_directory: &str,
        publication_directory: &str,
        attempt_directory: &str,
        inventory_path: &str,
        max_bytes: usize,
    ) -> Result<Vec<u8>, OrchestrationError> {
        if max_bytes == 0 {
            return Err(protocol_error(
                "output inventory byte limit must be positive",
            ));
        }

        let mut fs = self.client.fs();
        for (path, label) in [
            (control_directory, "output inventory control directory"),
            (
                publication_directory,
                "output inventory publication directory",
            ),
            (attempt_directory, "output inventory attempt directory"),
        ] {
            let metadata = fs.symlink_metadata(path).await.map_err(|error| {
                output_inventory_sftp_error("inspect handoff directory", &error)
            })?;
            ensure_directory_named(&metadata, label)?;
        }
        let path_metadata = fs
            .symlink_metadata(inventory_path)
            .await
            .map_err(|error| output_inventory_sftp_error("inspect output inventory", &error))?;
        ensure_output_inventory_file(&path_metadata, "output inventory")?;
        let expected_size = bounded_non_empty_size(&path_metadata, max_bytes, "output inventory")?;
        drop(fs);

        let mut file = self
            .client
            .options()
            .read(true)
            .open(inventory_path)
            .await
            .map_err(|error| {
                output_inventory_changed_sftp_error("open output inventory", &error)
            })?;
        let opened_metadata = file.metadata().await.map_err(|error| {
            output_inventory_changed_sftp_error("inspect opened output inventory", &error)
        })?;
        ensure_output_inventory_file(&opened_metadata, "opened output inventory")?;
        let opened_size =
            bounded_non_empty_size(&opened_metadata, max_bytes, "opened output inventory")?;
        if opened_size != expected_size {
            return Err(inventory_rejected(
                "output inventory changed between path inspection and open",
            ));
        }

        let content = read_bounded_inventory(&mut file, opened_size).await?;
        if content.len() != opened_size {
            return Err(inventory_rejected(
                "output inventory size changed while it was being read",
            ));
        }
        file.seek(std::io::SeekFrom::Start(0))
            .await
            .map_err(|error| output_inventory_io_error("rewind output inventory", &error))?;
        let verification = read_bounded_inventory(&mut file, opened_size).await?;
        if verification != content {
            return Err(inventory_rejected(
                "output inventory content changed while it was being read",
            ));
        }
        let final_metadata = file.metadata().await.map_err(|error| {
            output_inventory_changed_sftp_error("reinspect output inventory", &error)
        })?;
        ensure_output_inventory_file(&final_metadata, "output inventory")?;
        if bounded_non_empty_size(&final_metadata, max_bytes, "output inventory")? != opened_size {
            return Err(inventory_rejected(
                "output inventory size changed while it was being read",
            ));
        }
        file.close().await.map_err(|error| {
            output_inventory_changed_sftp_error("close output inventory", &error)
        })?;

        let mut fs = self.client.fs();
        for (path, label) in [
            (control_directory, "output inventory control directory"),
            (
                publication_directory,
                "output inventory publication directory",
            ),
            (attempt_directory, "output inventory attempt directory"),
        ] {
            let metadata = fs.symlink_metadata(path).await.map_err(|error| {
                output_inventory_changed_sftp_error("reinspect handoff directory", &error)
            })?;
            ensure_directory_named(&metadata, label)?;
        }
        let final_path_metadata = fs.symlink_metadata(inventory_path).await.map_err(|error| {
            output_inventory_changed_sftp_error("reinspect output inventory path", &error)
        })?;
        ensure_output_inventory_file(&final_path_metadata, "output inventory")?;
        if bounded_non_empty_size(&final_path_metadata, max_bytes, "output inventory")?
            != opened_size
        {
            return Err(inventory_rejected(
                "output inventory path changed while it was being read",
            ));
        }
        Ok(content)
    }

    pub(crate) async fn shutdown(self) -> Result<(), OrchestrationError> {
        self.client
            .close()
            .await
            .map_err(|error| sftp_error("close SFTP session", &error))
    }
}

async fn read_bounded_inventory(
    file: &mut File,
    expected_size: usize,
) -> Result<Vec<u8>, OrchestrationError> {
    file.read_all(expected_size, Default::default())
        .await
        .map(|content| content.to_vec())
        .map_err(|error| output_inventory_changed_sftp_error("read output inventory", &error))
}

fn ensure_directory_named(metadata: &MetaData, label: &str) -> Result<(), OrchestrationError> {
    match metadata.file_type() {
        Some(file_type) if file_type.is_dir() => ensure_output_mode(
            metadata
                .permissions()
                .map(|permissions| permissions.as_raw().bits()),
            OUTPUT_HANDOFF_DIRECTORY_MODE,
            label,
        ),
        _ => Err(inventory_rejected(&format!(
            "{label} is not a directory; refusing to follow it"
        ))),
    }
}

fn bounded_non_empty_size(
    metadata: &MetaData,
    max_bytes: usize,
    label: &str,
) -> Result<usize, OrchestrationError> {
    validate_output_size(metadata.len(), max_bytes, label)
}

fn validate_output_size(
    observed: Option<u64>,
    max_bytes: usize,
    label: &str,
) -> Result<usize, OrchestrationError> {
    let observed =
        observed.ok_or_else(|| inventory_rejected("SFTP server omitted output inventory size"))?;
    let max_bytes = u64::try_from(max_bytes)
        .map_err(|_| protocol_error("output inventory byte limit is unsupported"))?;
    if observed == 0 {
        return Err(inventory_rejected(&format!("{label} is empty")));
    }
    if observed > max_bytes {
        return Err(inventory_rejected(&format!(
            "{label} exceeds the 32 MiB limit"
        )));
    }
    usize::try_from(observed)
        .map_err(|_| inventory_rejected("output inventory exceeds the supported size"))
}

fn ensure_regular_file(metadata: &MetaData, label: &str) -> Result<(), OrchestrationError> {
    match metadata.file_type() {
        Some(file_type) if file_type.is_file() => Ok(()),
        _ => Err(protocol_error(&format!(
            "{label} is not a regular file; refusing to follow it"
        ))),
    }
}

fn ensure_output_inventory_file(
    metadata: &MetaData,
    label: &str,
) -> Result<(), OrchestrationError> {
    match metadata.file_type() {
        Some(file_type) if file_type.is_file() => ensure_output_mode(
            metadata
                .permissions()
                .map(|permissions| permissions.as_raw().bits()),
            OUTPUT_INVENTORY_FILE_MODE,
            label,
        ),
        _ => Err(inventory_rejected(&format!(
            "{label} is not a regular file; refusing to follow it"
        ))),
    }
}

fn ensure_output_mode(
    observed: Option<u32>,
    expected: u32,
    label: &str,
) -> Result<(), OrchestrationError> {
    let observed = observed
        .ok_or_else(|| inventory_rejected("SFTP server omitted output handoff permissions"))?
        & 0o7777;
    if observed == expected {
        Ok(())
    } else {
        Err(inventory_rejected(&format!(
            "{label} permissions are not {expected:04o}"
        )))
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
    let expected =
        u64::try_from(expected).map_err(|_| protocol_error("file exceeds the supported size"))?;
    match metadata.len() {
        Some(observed) if observed == expected => Ok(()),
        Some(_) => Err(protocol_error(&format!(
            "{label} size does not match the uploaded content",
        ))),
        None => Err(protocol_error("SFTP server omitted file size")),
    }
}

fn artifact_mode(mode: u32) -> Result<u16, OrchestrationError> {
    match mode {
        // Artifacts are never writable or executable by group/other. These
        // cover private configuration, shared-project reads, and public reads.
        0o600 | 0o640 | 0o644 => Ok(mode as u16),
        _ => Err(protocol_error(
            "submission artifact mode must be 0600, 0640, or 0644",
        )),
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

fn output_inventory_sftp_error(operation: &str, error: &SftpError) -> OrchestrationError {
    if is_missing(error) {
        OrchestrationError::OutputInventoryNotReady
    } else {
        sftp_error(operation, error)
    }
}

fn output_inventory_changed_sftp_error(operation: &str, error: &SftpError) -> OrchestrationError {
    if is_missing(error) {
        inventory_rejected("output inventory disappeared while it was being read")
    } else {
        sftp_error(operation, error)
    }
}

fn output_inventory_io_error(operation: &str, _error: &std::io::Error) -> OrchestrationError {
    OrchestrationError::Backend(format!("SFTP transfer: {operation}: SFTP transport failed"))
}

fn protocol_error(message: &str) -> OrchestrationError {
    OrchestrationError::Backend(format!("SFTP transfer: {message}"))
}

fn inventory_rejected(message: &str) -> OrchestrationError {
    OrchestrationError::OutputInventoryRejected(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn handoff_metadata_policy_and_artifact_modes_fail_closed() {
        assert!(ensure_output_mode(Some(0o700), 0o700, "directory").is_ok());
        assert!(ensure_output_mode(Some(0o600), 0o600, "file").is_ok());
        for (observed, expected) in [(Some(0o750), 0o700), (Some(0o640), 0o600), (None, 0o600)] {
            assert!(matches!(
                ensure_output_mode(observed, expected, "handoff"),
                Err(OrchestrationError::OutputInventoryRejected(_))
            ));
        }
        assert_eq!(validate_output_size(Some(1), 8, "inventory").unwrap(), 1);
        for size in [None, Some(0), Some(9)] {
            assert!(matches!(
                validate_output_size(size, 8, "inventory"),
                Err(OrchestrationError::OutputInventoryRejected(_))
            ));
        }
        for mode in [0o600, 0o640, 0o644] {
            assert_eq!(artifact_mode(mode).unwrap(), mode as u16);
        }
        for mode in [0, 0o400, 0o620, 0o660, 0o700, 0o755, 0o100600] {
            assert!(artifact_mode(mode).is_err(), "accepted {mode:04o}");
        }
    }

    #[test]
    fn server_controlled_errors_are_redacted() {
        static SECRET: &str = "do-not-log-this-token";
        let error = SftpError::InvalidResponse(&SECRET);
        let rendered = sftp_error("read output inventory", &error).to_string();
        assert!(!rendered.contains(SECRET));
        assert!(rendered.contains("invalid response"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn openssh_server_upload_and_bounded_inventory_read_enforce_policy() {
        let server = ["/usr/lib/openssh/sftp-server", "/usr/lib/ssh/sftp-server"]
            .into_iter()
            .find(|path| std::path::Path::new(path).is_file());
        let Some(server) = server else {
            eprintln!("skipping local SFTP integration: sftp-server is unavailable");
            return;
        };

        let root = tempfile::tempdir().unwrap();
        let control_directory = root.path().join(".beampipe");
        let publication_directory = control_directory.join("publication");
        let attempt_directory = publication_directory.join("attempt-0");
        std::fs::create_dir(&control_directory).unwrap();
        std::fs::create_dir(&publication_directory).unwrap();
        std::fs::create_dir(&attempt_directory).unwrap();
        std::fs::set_permissions(
            &control_directory,
            std::fs::Permissions::from_mode(OUTPUT_HANDOFF_DIRECTORY_MODE),
        )
        .unwrap();
        std::fs::set_permissions(
            &publication_directory,
            std::fs::Permissions::from_mode(OUTPUT_HANDOFF_DIRECTORY_MODE),
        )
        .unwrap();
        std::fs::set_permissions(
            &attempt_directory,
            std::fs::Permissions::from_mode(OUTPUT_HANDOFF_DIRECTORY_MODE),
        )
        .unwrap();
        let inventory_path = attempt_directory.join("beampipe-output-inventory.json");
        let inventory = br#"{"schema":"beampipe-output-inventory/v1"}"#;
        std::fs::write(&inventory_path, inventory).unwrap();
        std::fs::set_permissions(
            &inventory_path,
            std::fs::Permissions::from_mode(OUTPUT_INVENTORY_FILE_MODE),
        )
        .unwrap();
        let artifact_path = root.path().join("submission.graph");
        let artifact_temporary_path = root.path().join("submission.graph.tmp-test");
        let artifact = b"{\"graph\":true}";

        let mut child = tokio::process::Command::new(server)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut sftp = RemoteSftp::connect_parts(stdin, stdout).await.unwrap();

        sftp.upload_file_atomic(
            artifact_path.to_str().unwrap(),
            artifact_temporary_path.to_str().unwrap(),
            artifact,
            0o640,
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&artifact_path).unwrap(), artifact);
        assert!(!artifact_temporary_path.exists());
        assert_eq!(
            std::fs::metadata(&artifact_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o640,
        );

        assert_eq!(
            sftp.read_output_inventory(
                control_directory.to_str().unwrap(),
                publication_directory.to_str().unwrap(),
                attempt_directory.to_str().unwrap(),
                inventory_path.to_str().unwrap(),
                1024,
            )
            .await
            .unwrap(),
            inventory
        );

        std::fs::set_permissions(
            &publication_directory,
            std::fs::Permissions::from_mode(0o750),
        )
        .unwrap();
        let error = sftp
            .read_output_inventory(
                control_directory.to_str().unwrap(),
                publication_directory.to_str().unwrap(),
                attempt_directory.to_str().unwrap(),
                inventory_path.to_str().unwrap(),
                1024,
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            OrchestrationError::OutputInventoryRejected(_)
        ));
        std::fs::set_permissions(
            &publication_directory,
            std::fs::Permissions::from_mode(OUTPUT_HANDOFF_DIRECTORY_MODE),
        )
        .unwrap();

        std::fs::set_permissions(&attempt_directory, std::fs::Permissions::from_mode(0o750))
            .unwrap();
        let error = sftp
            .read_output_inventory(
                control_directory.to_str().unwrap(),
                publication_directory.to_str().unwrap(),
                attempt_directory.to_str().unwrap(),
                inventory_path.to_str().unwrap(),
                1024,
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            OrchestrationError::OutputInventoryRejected(_)
        ));
        std::fs::set_permissions(
            &attempt_directory,
            std::fs::Permissions::from_mode(OUTPUT_HANDOFF_DIRECTORY_MODE),
        )
        .unwrap();

        std::fs::set_permissions(&inventory_path, std::fs::Permissions::from_mode(0o640)).unwrap();
        let error = sftp
            .read_output_inventory(
                control_directory.to_str().unwrap(),
                publication_directory.to_str().unwrap(),
                attempt_directory.to_str().unwrap(),
                inventory_path.to_str().unwrap(),
                1024,
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            OrchestrationError::OutputInventoryRejected(_)
        ));
        std::fs::set_permissions(
            &inventory_path,
            std::fs::Permissions::from_mode(OUTPUT_INVENTORY_FILE_MODE),
        )
        .unwrap();

        std::fs::remove_file(&inventory_path).unwrap();
        std::os::unix::fs::symlink(&artifact_path, &inventory_path).unwrap();
        let error = sftp
            .read_output_inventory(
                control_directory.to_str().unwrap(),
                publication_directory.to_str().unwrap(),
                attempt_directory.to_str().unwrap(),
                inventory_path.to_str().unwrap(),
                1024,
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            OrchestrationError::OutputInventoryRejected(_)
        ));
        std::fs::remove_file(&inventory_path).unwrap();

        let error = sftp
            .read_output_inventory(
                control_directory.to_str().unwrap(),
                publication_directory.to_str().unwrap(),
                attempt_directory.to_str().unwrap(),
                inventory_path.to_str().unwrap(),
                1024,
            )
            .await
            .unwrap_err();
        assert!(matches!(error, OrchestrationError::OutputInventoryNotReady));

        std::fs::write(&inventory_path, []).unwrap();
        std::fs::set_permissions(
            &inventory_path,
            std::fs::Permissions::from_mode(OUTPUT_INVENTORY_FILE_MODE),
        )
        .unwrap();
        let error = sftp
            .read_output_inventory(
                control_directory.to_str().unwrap(),
                publication_directory.to_str().unwrap(),
                attempt_directory.to_str().unwrap(),
                inventory_path.to_str().unwrap(),
                1024,
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            OrchestrationError::OutputInventoryRejected(_)
        ));

        let oversized = std::fs::OpenOptions::new()
            .write(true)
            .open(&inventory_path)
            .unwrap();
        oversized.set_len(1025).unwrap();
        let error = sftp
            .read_output_inventory(
                control_directory.to_str().unwrap(),
                publication_directory.to_str().unwrap(),
                attempt_directory.to_str().unwrap(),
                inventory_path.to_str().unwrap(),
                1024,
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            OrchestrationError::OutputInventoryRejected(_)
        ));

        sftp.shutdown().await.unwrap();
        let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(status.success());
    }
}
