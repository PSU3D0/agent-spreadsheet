//! Artifact resolution for handles produced by canonical `screenshot_sheet`.
//!
//! Handles are opaque, content-addressed, and path-free: `artifact:sha256:<64 hex>`.
//! Resolution is a transport concern (MCP image content, HTTP artifact bytes); it
//! never accepts, exposes, or echoes a filesystem path.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Ceiling shared with `persist_png_artifact` in `agent_spreadsheet::canonical_optional`.
pub const MAX_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;

const HANDLE_PREFIX: &str = "artifact:sha256:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactError {
    /// Handle is not a well-formed `artifact:sha256:<64 lowercase hex>` string.
    Malformed,
    /// No artifact with that handle was produced by this process' workspace, or
    /// the stored bytes do not hash to the handle.
    NotFound,
    /// Stored object exceeds the artifact ceiling.
    TooLarge,
}

impl ArtifactError {
    pub fn message(&self) -> &'static str {
        match self {
            Self::Malformed => "artifact handle must be 'artifact:sha256:<64 hex>'",
            Self::NotFound => "artifact handle is not known to this server",
            Self::TooLarge => "artifact exceeds its media-type size ceiling",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedArtifact {
    pub bytes: Vec<u8>,
    pub media_type: &'static str,
}

/// Parse the hex digest out of a well-formed handle.
pub fn parse_handle(handle: &str) -> Result<&str, ArtifactError> {
    let hex = handle
        .strip_prefix(HANDLE_PREFIX)
        .ok_or(ArtifactError::Malformed)?;
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ArtifactError::Malformed);
    }
    Ok(hex)
}

/// Canonical artifact directory for a workspace. Refuses symlinks and non-directories.
fn artifact_root(workspace_root: &Path) -> Result<PathBuf, ArtifactError> {
    let workspace = workspace_root
        .canonicalize()
        .map_err(|_| ArtifactError::NotFound)?;
    let root = workspace.join("artifacts");
    let metadata = std::fs::symlink_metadata(&root).map_err(|_| ArtifactError::NotFound)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ArtifactError::NotFound);
    }
    let canonical = root.canonicalize().map_err(|_| ArtifactError::NotFound)?;
    if !canonical.starts_with(&workspace) {
        return Err(ArtifactError::NotFound);
    }
    Ok(canonical)
}

/// Resolve an artifact handle to bytes plus the recorded media type.
///
/// Refuses symlinks, anything outside `<workspace_root>/artifacts`, objects over
/// the 16 MiB ceiling, and files whose content does not hash to the handle.
pub fn resolve_artifact(
    workspace_root: &Path,
    handle: &str,
) -> Result<ResolvedArtifact, ArtifactError> {
    let (hex, extension, media_type, limit) = if let Some(hex) = handle.strip_prefix("artifact-") {
        if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err(ArtifactError::Malformed);
        }
        (hex, "xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", 64 * 1024 * 1024)
    } else {
        (parse_handle(handle)?, "png", "image/png", MAX_ARTIFACT_BYTES)
    };
    let root = artifact_root(workspace_root)?;
    let target = root.join(format!("{hex}.{extension}"));

    let metadata = std::fs::symlink_metadata(&target).map_err(|_| ArtifactError::NotFound)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ArtifactError::NotFound);
    }
    if metadata.len() > limit as u64 {
        return Err(ArtifactError::TooLarge);
    }
    let canonical = target.canonicalize().map_err(|_| ArtifactError::NotFound)?;
    if !canonical.starts_with(&root) {
        return Err(ArtifactError::NotFound);
    }

    let bytes = std::fs::read(&canonical).map_err(|_| ArtifactError::NotFound)?;
    if bytes.len() > limit {
        return Err(ArtifactError::TooLarge);
    }
    if format!("{:x}", Sha256::digest(&bytes)) != hex {
        return Err(ArtifactError::NotFound);
    }
    Ok(ResolvedArtifact {
        bytes,
        media_type,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_with(bytes: &[u8]) -> (tempfile::TempDir, String) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("artifacts");
        std::fs::create_dir_all(&root).unwrap();
        let hex = format!("{:x}", Sha256::digest(bytes));
        std::fs::write(root.join(format!("{hex}.png")), bytes).unwrap();
        (temp, format!("artifact:sha256:{hex}"))
    }

    #[test]
    fn resolves_a_well_formed_handle() {
        let (temp, handle) = workspace_with(b"stub-png-bytes");
        let resolved = resolve_artifact(temp.path(), &handle).unwrap();
        assert_eq!(resolved.bytes, b"stub-png-bytes");
        assert_eq!(resolved.media_type, "image/png");
    }

    #[test]
    fn rejects_malformed_handles() {
        let (temp, _) = workspace_with(b"stub");
        for handle in [
            "artifact:sha256:short",
            "sha256:0000",
            "artifact:sha256:../../etc/passwd",
            "artifact:sha256:ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789",
            "/etc/passwd",
        ] {
            assert_eq!(
                resolve_artifact(temp.path(), handle).unwrap_err(),
                ArtifactError::Malformed,
                "{handle}"
            );
        }
    }

    #[test]
    fn unknown_handle_is_not_found() {
        let (temp, _) = workspace_with(b"stub");
        let handle = format!("artifact:sha256:{}", "a".repeat(64));
        assert_eq!(
            resolve_artifact(temp.path(), &handle).unwrap_err(),
            ArtifactError::NotFound
        );
    }

    #[test]
    fn content_that_does_not_match_the_name_is_not_served() {
        let (temp, handle) = workspace_with(b"stub");
        let hex = parse_handle(&handle).unwrap().to_string();
        std::fs::write(
            temp.path().join("artifacts").join(format!("{hex}.png")),
            b"tampered",
        )
        .unwrap();
        assert_eq!(
            resolve_artifact(temp.path(), &handle).unwrap_err(),
            ArtifactError::NotFound
        );
    }

    #[test]
    fn symlinked_artifact_is_not_served() {
        let (temp, _) = workspace_with(b"stub");
        let secret = temp.path().join("secret.png");
        std::fs::write(&secret, b"secret").unwrap();
        let hex = format!("{:x}", Sha256::digest(b"secret"));
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            &secret,
            temp.path().join("artifacts").join(format!("{hex}.png")),
        )
        .unwrap();
        #[cfg(unix)]
        assert_eq!(
            resolve_artifact(temp.path(), &format!("artifact:sha256:{hex}")).unwrap_err(),
            ArtifactError::NotFound
        );
    }
}
