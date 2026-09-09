//! Filesystem policy for the private host's explicit materialize control.
use crate::{
    canonical_lifecycle::ArtifactMetadata,
    native_resident::{
        MAX_SOURCE_BYTES, NativeBinding, check_private, create_private_directory, sync_directory,
    },
    resident_export::{FileExportIntent, FileExportPlan, FileExportSink},
};
use anyhow::{Context, Result, ensure};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub(crate) struct NativeFileExportSink {
    pub binding: NativeBinding,
    pub directory: PathBuf,
}
fn regular_bytes(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() <= limit,
        "export path must be a bounded regular file"
    );
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    ensure!(file.metadata()?.is_file(), "export file changed type");
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "export file exceeds limit");
    Ok(bytes)
}
fn destination(path: &str) -> Result<PathBuf> {
    let path = PathBuf::from(path);
    ensure!(path.is_absolute(), "export path must be absolute");
    let parent = path.parent().context("export path lacks parent")?;
    ensure!(
        parent.canonicalize()? == parent,
        "export parent must be canonical (no symlink traversal)"
    );
    for ancestor in parent.ancestors() {
        let metadata = fs::symlink_metadata(ancestor)?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "export ancestors must be real directories"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let uid = unsafe { libc::geteuid() };
            ensure!(
                metadata.uid() == uid || metadata.uid() == 0,
                "untrusted export ancestor"
            );
            ensure!(
                metadata.permissions().mode() & 0o022 == 0
                    || (metadata.uid() == 0 && metadata.permissions().mode() & 0o1000 != 0),
                "export ancestor is writable by others"
            );
        }
    }
    if path.try_exists()? {
        let meta = fs::symlink_metadata(&path)?;
        ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "export destination must be a regular file"
        );
    }
    Ok(path)
}
impl NativeFileExportSink {
    fn capture_root(&self) -> Result<PathBuf> {
        let root = self.directory.join("captures");
        if !root.try_exists()? {
            create_private_directory(&root)?;
            sync_directory(&self.directory)?;
        }
        check_private(&root, true)?;
        Ok(root)
    }
    fn verify_source(&self) -> Result<()> {
        let bytes = regular_bytes(&self.binding.source, MAX_SOURCE_BYTES)
            .context("source changed or disappeared; use save-as or explicit --force")?;
        ensure!(
            crate::utils::hash_bytes_sha256_hex(&bytes) == self.binding.source_sha256,
            "source changed; use save-as or explicit --force"
        );
        Ok(())
    }
}
fn destination_generation(path: &std::path::Path) -> Result<String> {
    if !path.try_exists()? {
        return Ok("missing".into());
    }
    Ok(format!(
        "sha256:{}",
        crate::utils::hash_bytes_sha256_hex(&regular_bytes(path, MAX_SOURCE_BYTES)?)
    ))
}

#[async_trait::async_trait(?Send)]
impl FileExportSink for NativeFileExportSink {
    async fn authorize(&self, intent: &FileExportIntent) -> Result<(String, String, String)> {
        let path = destination(
            intent.destination.as_deref().unwrap_or(
                self.binding
                    .source
                    .to_str()
                    .context("source path is not UTF-8")?,
            ),
        )?;
        if intent.destination.is_some() {
            ensure!(
                path != self.binding.source,
                "source replacement requires --source, not --output"
            );
            ensure!(
                intent.force || !path.try_exists()?,
                "output exists; use --force to replace it"
            );
        } else if !intent.force {
            self.verify_source()?;
        }
        Ok((
            path.to_str().context("export path is not UTF-8")?.into(),
            self.binding.source_sha256.clone(),
            destination_generation(&path)?,
        ))
    }
    async fn retain(&self, bytes: &[u8]) -> Result<ArtifactMetadata> {
        ensure!(
            bytes.len() as u64 <= MAX_SOURCE_BYTES,
            "capture exceeds byte limit"
        );
        let root = self.capture_root()?;
        let hash = crate::utils::hash_bytes_sha256_hex(bytes);
        let path = root.join(format!("{hash}.xlsx"));
        if !path.try_exists()? {
            let used = fs::read_dir(&root)?.try_fold(0u64, |sum, entry| -> Result<u64> {
                let entry = entry?;
                let metadata = entry.metadata()?;
                ensure!(
                    entry.file_type()?.is_file(),
                    "unexpected capture directory entry"
                );
                Ok(sum.saturating_add(metadata.len()))
            })?;
            ensure!(
                used.saturating_add(bytes.len() as u64) <= 128 * 1024 * 1024,
                "retained capture lifetime byte quota reached (including orphans); no automatic history expiry"
            );
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
            }
            let mut file = options.open(&path)?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        let retained = regular_bytes(&path, MAX_SOURCE_BYTES)?;
        ensure!(
            retained == bytes,
            "capture hash collision or incomplete preparation"
        );
        OpenOptions::new().read(true).open(&path)?.sync_all()?;
        sync_directory(&root)?;
        Ok(ArtifactMetadata {
            artifact_id: format!("artifact-{hash}"),
            media_type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".into(),
            bytes: bytes.len() as u64,
            sha256: hash,
        })
    }
    async fn publish(&self, plan: &FileExportPlan) -> Result<()> {
        ensure!(
            plan.source_sha256 == self.binding.source_sha256,
            "export source authority mismatch"
        );
        let bytes = regular_bytes(
            &self
                .capture_root()?
                .join(format!("{}.xlsx", plan.artifact.sha256)),
            MAX_SOURCE_BYTES,
        )?;
        ensure!(
            bytes.len() as u64 == plan.artifact.bytes
                && crate::utils::hash_bytes_sha256_hex(&bytes) == plan.artifact.sha256,
            "retained capture mismatch"
        );
        let path = destination(&plan.destination)?;
        let authorized = plan
            .intent
            .destination
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.binding.source.clone());
        ensure!(
            path == authorized,
            "prepared path differs from authorized destination"
        );
        ensure!(
            plan.intent.destination.is_none() || path != self.binding.source,
            "source replacement requires explicit source intent"
        );
        if path.try_exists()? {
            let existing = regular_bytes(&path, MAX_SOURCE_BYTES)?;
            if existing == bytes {
                OpenOptions::new().read(true).open(&path)?.sync_all()?;
                sync_directory(path.parent().unwrap())?;
                return Ok(());
            }
        }
        self.authorize(&plan.intent).await?;
        let expected_generation = plan
            .destination_generation
            .as_deref()
            .context("old pending export lacks destination generation; publication refused")?;
        ensure!(
            destination_generation(&path)? == expected_generation,
            "export destination changed since first admission; pending capture will not overwrite newer data"
        );
        let parent = path.parent().unwrap();
        let temporary = parent.join(format!(".asp-export-{}", uuid::Uuid::new_v4().simple()));
        let result: Result<()> = (|| {
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            if plan.intent.destination.is_none() && !plan.intent.force {
                self.verify_source()?;
            }
            ensure!(
                destination_generation(&path)? == expected_generation,
                "export destination changed during publication"
            );
            if plan.intent.force || plan.intent.destination.is_none() {
                fs::rename(&temporary, &path)?;
            } else {
                fs::hard_link(&temporary, &path)?;
                fs::remove_file(&temporary)?;
            }
            sync_directory(parent)?;
            Ok(())
        })();
        let _ = fs::remove_file(&temporary);
        result.context(
            "filesystem publication may have committed; atomic rename is not filesystem CAS",
        )
    }
}
