//! Native serialized owner lanes. Spreadsheet execution stays in the canonical dispatcher.
//!
//! The transport accepts bounded messages, then transfers ownership to a lane. Dropping a
//! response receiver never cancels accepted work. Each evaluator is constructed and dropped
//! on that lane; no Send/Sync assertion is made for a workbook or evaluator.
use crate::{
    canonical_write::recover_durable_resident_session,
    config::ServerConfig,
    core::resident_storage::native::NativeResidentJournal,
    operations::{CanonicalErrorEnvelope, CanonicalResponse, decode_operation},
    session::ResidentSessionRuntime,
};
use anyhow::{Context, Result, bail, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::{mpsc, oneshot};

pub const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;
pub const LANE_QUEUE_CAPACITY: usize = 16;
/// Inactive preparations count too: failed creations cannot bypass disk admission.
pub const MAX_RESOURCE_DIRECTORIES: usize = 128;
const BINDING_VERSION: u32 = 2;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeBinding {
    version: u32,
    pub resource_id: String,
    pub source: PathBuf,
    pub source_sha256: String,
    pub source_bytes: u64,
    #[serde(default)]
    pub base_sha256: Option<String>,
    #[serde(default)]
    pub base_bytes: Option<u64>,
    pub config: ServerConfig,
}

/// Holds the exclusive interprocess owner lock for the whole lifetime of a lane.
/// Journal locks alone do not exclude two live evaluators with different revisions.
struct NativeArtifactSink {
    workspace: PathBuf,
    source: PathBuf,
    source_sha256: String,
    source_bytes: u64,
}
#[async_trait::async_trait(?Send)]
impl crate::session::ResidentArtifactSink for NativeArtifactSink {
    async fn validate(
        &self,
        request: &crate::canonical_lifecycle::ExportForkRequest,
    ) -> Result<()> {
        crate::canonical_lifecycle::export_destination_name(&request.destination)?;
        let metadata = fs::symlink_metadata(&self.source)
            .context("revision conflict: bound export source is unavailable")?;
        ensure!(
            metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.len() == self.source_bytes,
            "revision conflict: bound export source changed"
        );
        ensure!(
            self.source.canonicalize()? == self.source && self.source.starts_with(&self.workspace),
            "revision conflict: bound export source path changed"
        );
        let file = private_options()
            .read(true)
            .open(&self.source)
            .context("revision conflict: bound export source is unavailable")?;
        let mut contents = Vec::new();
        file.take(self.source_bytes + 1)
            .read_to_end(&mut contents)?;
        let generation = crate::utils::hash_bytes_sha256_hex(&contents);
        ensure!(
            contents.len() as u64 == self.source_bytes && generation == self.source_sha256,
            "revision conflict: bound export source changed"
        );
        Ok(())
    }
    async fn publish(
        &self,
        request: &crate::canonical_lifecycle::ExportForkRequest,
        bytes: &[u8],
    ) -> Result<crate::canonical_lifecycle::ArtifactMetadata> {
        self.validate(request).await?;
        let hash = crate::utils::hash_bytes_sha256_hex(bytes);
        let destination_root = self.workspace.join("artifacts");
        match create_private_directory(&destination_root) {
            Ok(()) => (),
            Err(_) if destination_root.is_dir() => (), // Validate an existing namespace below.
            Err(error) => return Err(error.context("artifact root publication outcome unknown")),
        }
        let root = crate::canonical_lifecycle::artifact_root(&self.workspace)?;
        // Export targets are public artifacts, not private journal directories,
        // but their namespace must still be owned and nonreplaceable by others.
        for ancestor in root.ancestors() {
            let metadata = fs::symlink_metadata(ancestor)?;
            ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "artifact ancestor must be a real directory"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::{MetadataExt, PermissionsExt};
                // SAFETY: geteuid has no arguments or memory safety preconditions.
                let uid = unsafe { libc::geteuid() };
                ensure!(
                    metadata.uid() == uid || metadata.uid() == 0,
                    "artifact ancestor belongs to an untrusted user"
                );
                let mode = metadata.permissions().mode();
                if ancestor == root {
                    ensure!(
                        metadata.uid() == uid && mode & 0o022 == 0,
                        "artifact root must be owned by this user and not writable by others"
                    );
                }
                ensure!(
                    mode & 0o022 == 0 || (metadata.uid() == 0 && mode & 0o1000 != 0),
                    "artifact ancestor is replaceable by other users"
                );
            }
        }
        ensure!(
            bytes.len() as u64 <= MAX_SOURCE_BYTES,
            "artifact exceeds native byte admission limit"
        );
        let path = crate::canonical_lifecycle::persist_content_artifact(&root, &hash, bytes)
            .context("artifact publication outcome unknown")?;
        // Existing identical objects also require a reconciliation file barrier.
        private_options()
            .read(true)
            .open(path)
            .context("artifact file outcome unknown")?
            .sync_all()
            .context("artifact file barrier outcome unknown")?;
        sync_directory(&root).context("artifact directory barrier outcome unknown")?;
        sync_directory(&self.workspace).context("artifact root publication outcome unknown")?;
        Ok(crate::canonical_lifecycle::ArtifactMetadata {
            artifact_id: format!("artifact-{hash}"),
            media_type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".into(),
            bytes: bytes.len() as u64,
            sha256: hash,
        })
    }
}

struct BoundOwner {
    _ownership: File,
    runtime: ResidentSessionRuntime<NativeResidentJournal>,
}

fn resource_key(resource: &str) -> Result<&str> {
    let _: crate::operations::ResourceId = serde_json::from_value(Value::String(resource.into()))?;
    let (kind, key) = resource
        .split_once(':')
        .context("typed resource identity required")?;
    ensure!(
        matches!(kind, "session" | "fork"),
        "durable resource requires session: or fork:"
    );
    ensure!(
        !key.is_empty()
            && !key.starts_with("pending_")
            && key.len() <= 128
            && key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
        "invalid durable resource key"
    );
    Ok(key)
}

pub(super) fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options
}

pub(super) fn check_private(path: &Path, directory: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        !metadata.file_type().is_symlink()
            && metadata.is_dir() == directory
            && (directory || metadata.is_file()),
        "native state must be a real {}",
        if directory { "directory" } else { "file" }
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        ensure!(
            metadata.permissions().mode() & 0o077 == 0,
            "native state must be private"
        );
        // SAFETY: geteuid has no arguments or memory safety preconditions.
        ensure!(
            metadata.uid() == unsafe { libc::geteuid() },
            "native state must belong to this user"
        );
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "native state must not be a reparse point"
        );
        bail!("Windows native binding ACL validation is not implemented yet");
    }
    Ok(())
}

pub(super) fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

pub(super) fn create_private_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(path)?;
    }
    #[cfg(not(unix))]
    fs::create_dir(path)?;
    check_private(path, true)?;
    sync_directory(path)?;
    sync_directory(path.parent().context("native directory needs a parent")?)
}

pub(super) fn read_private(path: &Path, limit: u64) -> Result<Vec<u8>> {
    check_private(path, false)?;
    let file = private_options().read(true).open(path)?;
    ensure!(
        file.metadata()?.len() <= limit,
        "native file exceeds admission limit"
    );
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "native file exceeds admission limit"
    );
    Ok(bytes)
}

pub(super) fn write_new_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = private_options().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

/// A pre-provisioned private, durable, trusted root. No recursive mkdir and no
/// claim of protection against replacement by a hostile ancestor owner.
#[derive(Clone)]
pub struct NativeBindings {
    root: PathBuf,
}
impl NativeBindings {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        ensure!(root.is_absolute(), "native binding root must be absolute");
        check_private(root, true)?;
        // Refuse symlink traversal rather than canonicalizing it away. The caller
        // must additionally provision trusted/nonreplaceable ancestors.
        for ancestor in root.ancestors() {
            let metadata = fs::symlink_metadata(ancestor)?;
            ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "native binding ancestors must be real directories"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::{MetadataExt, PermissionsExt};
                // SAFETY: geteuid has no arguments or memory safety preconditions.
                let uid = unsafe { libc::geteuid() };
                ensure!(
                    metadata.uid() == 0 || metadata.uid() == uid,
                    "native ancestor belongs to an untrusted user"
                );
                let mode = metadata.permissions().mode();
                ensure!(
                    mode & 0o022 == 0 || (metadata.uid() == 0 && mode & 0o1000 != 0),
                    "native ancestor is replaceable by other users"
                );
            }
        }
        sync_directory(root)?;
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    fn directory(&self, resource: &str) -> Result<PathBuf> {
        let key = resource_key(resource)?;
        // Keep aliases from accidentally creating separate owners for one key.
        Ok(self.root.join(key))
    }

    /// Capture one immutable source generation, validate it with the actual
    /// workbook loader before activation, then persist base/config before any
    /// journal can refer to them. The activation rename is the only publication.
    pub fn create(
        &self,
        resource: &str,
        source: &Path,
        expected_sha256: &str,
        config: ServerConfig,
    ) -> Result<NativeBinding> {
        let (binding, staging, _lock) = self.prepare(resource, source, expected_sha256, config)?;
        fs::rename(&staging, self.directory(resource)?)?;
        sync_directory(&self.root).context("binding activation outcome unknown")?;
        Ok(binding)
    }

    fn prepare(
        &self,
        resource: &str,
        source: &Path,
        expected_sha256: &str,
        config: ServerConfig,
    ) -> Result<(NativeBinding, PathBuf, File)> {
        let source = source.canonicalize()?;
        ensure!(
            source.starts_with(config.workspace_root.canonicalize()?),
            "source is outside binding workspace"
        );
        let file = File::open(&source)?;
        ensure!(
            file.metadata()?.is_file() && file.metadata()?.len() <= MAX_SOURCE_BYTES,
            "source exceeds admission limit or is not a file"
        );
        let mut bytes = Vec::new();
        file.take(MAX_SOURCE_BYTES + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= MAX_SOURCE_BYTES,
            "source exceeds admission limit"
        );
        let hash = crate::utils::hash_bytes_sha256_hex(&bytes);
        ensure!(hash == expected_sha256, "source revision conflict");
        let binding = NativeBinding {
            version: BINDING_VERSION,
            resource_id: resource.into(),
            source,
            source_sha256: hash,
            source_bytes: bytes.len() as u64,
            base_sha256: None,
            base_bytes: None,
            config,
        };
        self.prepare_bytes(resource, bytes, binding)
    }

    fn prepare_bytes(
        &self,
        resource: &str,
        bytes: Vec<u8>,
        mut binding: NativeBinding,
    ) -> Result<(NativeBinding, PathBuf, File)> {
        ensure!(
            bytes.len() as u64 <= MAX_SOURCE_BYTES,
            "snapshot exceeds native byte admission limit"
        );
        let destination = self.directory(resource)?;
        // Inspect central-directory expansion before allowing the workbook loader
        // to allocate decompressed XML. Limits are admission, not an RSS promise.
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes))?;
        ensure!(archive.len() <= 10_000, "source has too many ZIP members");
        let mut expanded = 0u64;
        for index in 0..archive.len() {
            let entry = archive.by_index(index)?;
            let remaining = 256 * 1024 * 1024 - expanded;
            ensure!(
                entry.size() <= remaining,
                "source decompressed size exceeds admission limit"
            );
            let actual = std::io::copy(&mut entry.take(remaining + 1), &mut std::io::sink())?;
            ensure!(
                actual <= remaining,
                "source actual decompressed size exceeds admission limit"
            );
            expanded += actual;
        }
        drop(archive);
        // Validate the document without constructing an evaluator: the only
        // evaluator ingest belongs to the subsequently attached owning lane.
        let _ = crate::core::session::WorkbookSession::from_bytes(&bytes)?;
        binding.version = BINDING_VERSION;
        binding.resource_id = resource.into();
        binding.base_sha256 = Some(crate::utils::hash_bytes_sha256_hex(&bytes));
        binding.base_bytes = Some(bytes.len() as u64);
        let lock_path = self.root.join("catalog.lock");
        let lock = private_options()
            .create(true)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        check_private(&lock_path, false)?;
        lock.lock_exclusive()?;
        ensure!(!destination.try_exists()?, "resource already exists");
        let directories =
            fs::read_dir(&self.root)?.try_fold(0usize, |count, entry| -> Result<usize> {
                Ok(count + usize::from(entry?.file_type()?.is_dir()))
            })?;
        ensure!(
            directories < MAX_RESOURCE_DIRECTORIES,
            "durable resource quota reached (including inactive preparations)"
        );
        let staging = self
            .root
            .join(format!("pending_{}", uuid::Uuid::new_v4().simple()));
        create_private_directory(&staging)?;
        // Failed preparation is deliberately not activated. Orphans are safe to
        // inspect/remove offline; no age-based deletion of live resource authority.
        write_new_private(&staging.join("base.xlsx"), &bytes)?;
        write_new_private(
            &staging.join("binding.json"),
            &serde_json::to_vec(&binding)?,
        )?;
        create_private_directory(&staging.join("journal"))?;
        sync_directory(&staging)?;
        Ok((binding, staging, lock))
    }

    /// Enumerate only activated bindings. Pending directories are never a
    /// catalog entry, even when preparation contains an otherwise valid receipt.
    pub fn catalog_resources(&self) -> Result<Vec<String>> {
        let mut resources = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("invalid catalog directory"))?;
            if name.starts_with("pending_") {
                continue;
            }
            check_private(&entry.path(), true)?;
            let binding: NativeBinding = serde_json::from_slice(&read_private(
                &entry.path().join("binding.json"),
                1024 * 1024,
            )?)?;
            ensure!(
                self.directory(&binding.resource_id)? == entry.path(),
                "catalog binding directory mismatch"
            );
            self.binding(&binding.resource_id)?;
            if binding.resource_id.starts_with("fork:") {
                resources.push(binding.resource_id);
            }
        }
        resources.sort();
        Ok(resources)
    }

    pub async fn inactive_descriptor(
        &self,
        resource: &str,
    ) -> Result<Option<crate::canonical_lifecycle::CanonicalForkDescriptor>> {
        use crate::core::resident_storage::ResidentCommitStorage;
        let binding = self.binding(resource)?;
        let storage = NativeResidentJournal::open(self.directory(resource)?.join("journal"))?;
        let records = storage.load(resource_key(resource)?).await?;
        crate::core::resident_storage::PortableHistoryState::replay_bound(
            &records,
            resource_key(resource)?,
            binding
                .base_sha256
                .as_deref()
                .unwrap_or(&binding.source_sha256),
        )?;
        crate::session::resident_catalog_descriptor(
            &serde_json::from_value(Value::String(resource.into()))?,
            &records,
            None,
        )
    }

    /// Validated receipt-only attachment. This never opens an evaluator or an
    /// owner lane, and does not consume live-owner admission capacity.
    pub async fn terminal_journal(&self, resource: &str) -> Result<Option<NativeResidentJournal>> {
        let bindings = self.clone();
        let resource = resource.to_owned();
        tokio::task::spawn_blocking(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(bindings.terminal_journal_local(&resource))
        })
        .await?
    }
    async fn terminal_journal_local(
        &self,
        resource: &str,
    ) -> Result<Option<NativeResidentJournal>> {
        use crate::core::resident_storage::ResidentCommitStorage;
        let binding = self.binding(resource)?;
        let journal = NativeResidentJournal::open(self.directory(resource)?.join("journal"))?;
        let records = journal.load(resource_key(resource)?).await?;
        if records.is_empty() {
            return Ok(None);
        }
        let state = crate::core::resident_storage::PortableHistoryState::replay_bound(
            &records,
            resource_key(resource)?,
            binding
                .base_sha256
                .as_deref()
                .unwrap_or(&binding.source_sha256),
        )?;
        Ok(state.discarded.then_some(journal))
    }

    pub fn binding(&self, resource: &str) -> Result<NativeBinding> {
        Self::binding_at(&self.directory(resource)?, resource)
    }

    fn binding_at(directory: &Path, resource: &str) -> Result<NativeBinding> {
        check_private(directory, true)?;
        let binding: NativeBinding =
            serde_json::from_slice(&read_private(&directory.join("binding.json"), 1024 * 1024)?)?;
        ensure!(
            (binding.version == 1 && binding.base_sha256.is_none() && binding.base_bytes.is_none())
                || (binding.version == BINDING_VERSION
                    && binding.base_sha256.is_some()
                    && binding.base_bytes.is_some()),
            "unsupported or incomplete native binding version"
        );
        ensure!(
            binding.resource_id == resource,
            "resource binding identity mismatch"
        );
        Ok(binding)
    }

    async fn recover_at(directory: &Path, resource: &str) -> Result<BoundOwner> {
        let binding = Self::binding_at(directory, resource)?;
        let lock_path = directory.join("owner.lock");
        let ownership = private_options()
            .create(true)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        check_private(&lock_path, false)?;
        ownership
            .try_lock_exclusive()
            .context("resource already has a live owner")?;
        let base = read_private(&directory.join("base.xlsx"), MAX_SOURCE_BYTES)?;
        ensure!(
            crate::utils::hash_bytes_sha256_hex(&base)
                == binding
                    .base_sha256
                    .as_deref()
                    .unwrap_or(&binding.source_sha256)
                && base.len() as u64 == binding.base_bytes.unwrap_or(binding.source_bytes),
            "immutable base binding mismatch"
        );
        let storage = NativeResidentJournal::open(directory.join("journal"))?;
        let core_resource = format!("session:{}", resource_key(resource)?);
        let owner = recover_durable_resident_session(&core_resource, &base, &storage).await?;
        let sink = NativeArtifactSink {
            workspace: binding.config.workspace_root.clone(),
            source: binding.source.clone(),
            source_sha256: binding.source_sha256.clone(),
            source_bytes: binding.source_bytes,
        };
        let runtime = ResidentSessionRuntime::new(
            owner,
            storage,
            Arc::new(binding.config),
            serde_json::from_value(Value::String(resource.into()))?,
        )?
        .with_artifact_sink(sink);
        Ok(BoundOwner {
            _ownership: ownership,
            runtime,
        })
    }

    pub async fn file_export_outcome(
        &self,
        intent: &crate::resident_export::FileExportIntent,
    ) -> Result<Option<crate::resident_export::FileExportResult>> {
        let bindings = self.clone();
        let intent = intent.clone();
        tokio::task::spawn_blocking(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(bindings.file_export_outcome_local(&intent))
        })
        .await?
    }
    async fn file_export_outcome_local(
        &self,
        intent: &crate::resident_export::FileExportIntent,
    ) -> Result<Option<crate::resident_export::FileExportResult>> {
        use crate::core::resident_storage::{
            PortableHistoryState, ReconcileOutcome, ResidentCommitStorage,
        };
        let binding = self.binding(&intent.resource_id)?;
        let storage =
            NativeResidentJournal::open(self.directory(&intent.resource_id)?.join("journal"))?;
        let key = resource_key(&intent.resource_id)?;
        let records = storage.load(key).await?;
        let history = PortableHistoryState::replay_bound(
            &records,
            key,
            binding
                .base_sha256
                .as_deref()
                .unwrap_or(&binding.source_sha256),
        )?;
        if let Some(plan) = history.file_export_plans.get(&intent.request_id) {
            ensure!(
                &plan.intent == intent,
                "file export request identity reuse with different input"
            );
        }
        if let Some(result) = history.file_export_results.get(&intent.request_id) {
            let record = records
                .iter()
                .find(|record| record.request_id == intent.request_id)
                .context("file export result lacks receipt")?;
            ensure!(
                matches!(
                    storage
                        .reconcile(key, &intent.request_id, &record.request_fingerprint)
                        .await?,
                    ReconcileOutcome::Committed(_)
                ),
                "file export receipt outcome unknown"
            );
            return Ok(Some(result.clone()));
        }
        Ok(None)
    }

    pub fn creation_resource(
        request_id: &str,
        base: &crate::operations::ResourceId,
    ) -> Result<String> {
        ensure!(
            !request_id.is_empty() && request_id.len() <= 256,
            "invalid creation request identity"
        );
        let key = crate::utils::hash_bytes_sha256_hex(&serde_json::to_vec(&(base, request_id))?);
        Ok(format!("fork:created_{key}"))
    }

    pub async fn creation_outcome(
        &self,
        request_id: &str,
        request: &crate::canonical_lifecycle::CreateForkRequest,
    ) -> Result<Option<CanonicalResponse>> {
        let resource = Self::creation_resource(request_id, &request.resource_id)?;
        let directory = self.directory(&resource)?;
        if !directory.try_exists()? {
            return Ok(None);
        }
        let binding = self
            .binding(&resource)
            .context("creation binding requires recovery")?;
        // Reconcile activation as well as journal barriers before claiming success.
        sync_directory(&self.root).context("creation activation outcome unknown")?;
        let storage = NativeResidentJournal::open(directory.join("journal"))
            .context("creation journal requires recovery")?;
        let response = crate::session::reconcile_creation(
            &storage,
            &serde_json::from_value(Value::String(resource))?,
            binding
                .base_sha256
                .as_deref()
                .unwrap_or(&binding.source_sha256),
            request_id,
            request,
        )
        .await
        .map_err(|error| {
            if error.to_string().starts_with("request identity reuse") {
                error
            } else {
                error.context("creation reconciliation outcome unknown")
            }
        })?;
        Ok(Some(response))
    }

    /// Returns the exact still-live creation owner, not a recovered replacement.
    pub async fn create_canonical(
        &self,
        request_id: String,
        request: crate::canonical_lifecycle::CreateForkRequest,
        source: PathBuf,
        expected_sha256: String,
        config: ServerConfig,
    ) -> Result<(NativeOwnerLane, CanonicalResponse)> {
        let resource = Self::creation_resource(&request_id, &request.resource_id)?;
        let store = self.clone();
        let key = resource.clone();
        let (_, pending, lock) = tokio::task::spawn_blocking(move || {
            store.prepare(&key, &source, &expected_sha256, config)
        })
        .await??;
        let (lane, response) = self
            .start_lane(&resource, pending, Some((request_id, request, lock)))
            .await?;
        Ok((lane, response.context("creation response missing")?))
    }

    pub async fn create_child(
        &self,
        request_id: String,
        request: crate::canonical_lifecycle::CreateForkRequest,
        parent: NativeBinding,
        bytes: Vec<u8>,
    ) -> Result<(NativeOwnerLane, CanonicalResponse)> {
        let resource = Self::creation_resource(&request_id, &request.resource_id)?;
        let store = self.clone();
        let key = resource.clone();
        let (_, pending, lock) =
            tokio::task::spawn_blocking(move || store.prepare_bytes(&key, bytes, parent)).await??;
        let (lane, response) = self
            .start_lane(&resource, pending, Some((request_id, request, lock)))
            .await?;
        Ok((lane, response.context("child creation response missing")?))
    }

    /// Construct the owner on a dedicated thread, never on the caller's runtime.
    pub async fn attach(&self, resource: &str) -> Result<NativeOwnerLane> {
        self.start_lane(resource, self.directory(resource)?, None)
            .await
            .map(|(lane, _)| lane)
    }

    async fn start_lane(
        &self,
        resource: &str,
        directory: PathBuf,
        creation: Option<(String, crate::canonical_lifecycle::CreateForkRequest, File)>,
    ) -> Result<(NativeOwnerLane, Option<CanonicalResponse>)> {
        Self::binding_at(&directory, resource)?;
        let destination = self.directory(resource)?;
        let root = self.root.clone();
        let resource = resource.to_owned();
        let (tx, mut rx) = mpsc::channel::<LaneRequest>(LANE_QUEUE_CAPACITY);
        let (ready_tx, ready_rx) = oneshot::channel();
        let active_request = Arc::new(std::sync::Mutex::new(None::<String>));
        let lane_active = active_request.clone();
        let thread = std::thread::Builder::new().name(format!("resident-{resource}")).spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread().enable_all().build();
            let executor = match result { Ok(value) => value, Err(error) => { let _ = ready_tx.send(Err(error.to_string())); return; } };
            executor.block_on(async move {
                let mut owner = match Self::recover_at(&directory, &resource).await {
                    Ok(owner) => owner,
                    Err(error) => { let _ = ready_tx.send(Err(error.to_string())); return; }
                };
                let prepared = if let Some((request_id, request, _catalog_lock)) = creation {
                    let result: Result<CanonicalResponse> = async {
                        let response = owner.runtime.prepare_creation(&request_id, request).await?;
                        fs::rename(&directory, &destination)?;
                        sync_directory(&root).context("creation activation outcome unknown")?;
                        // Retain the exact owner/epoch; only relocate host storage.
                        owner.runtime.storage = NativeResidentJournal::open(destination.join("journal"))
                            .context("creation storage rebinding outcome unknown")?;
                        Ok(response)
                    }.await;
                    match result {
                        Ok(response) => Some(response),
                        Err(error) => { let _ = ready_tx.send(Err(error.to_string())); return; }
                    }
                } else { None };
                let _ = ready_tx.send(Ok(prepared));
                while let Some(request) = rx.recv().await {
                    match request {
                        LaneRequest::Execute { request_id, operation, payload, verification_baseline, reply } => {
                            *lane_active.lock().expect("admission diagnostic mutex") = Some(request_id.clone());
                            let response = match decode_operation(&operation, payload) {
                                Ok(operation) => owner.runtime.execute_with_verification_baseline(&request_id, operation, verification_baseline).await,
                                Err(error) => Err(error),
                            };
                            *lane_active.lock().expect("admission diagnostic mutex") = None;
                            // Close admission at a terminal receipt, but drain every
                            // request already accepted before dropping the evaluator.
                            if owner.runtime.is_discarded() { rx.close(); }
                            // Accepted work finishes even when the client has disconnected.
                            let _ = reply.send(response);
                        }
                        LaneRequest::Materialize { intent, reply } => {
                            *lane_active.lock().expect("admission diagnostic mutex") = Some(intent.request_id.clone());
                            let result = async {
                                let binding = Self::binding_at(&destination, &resource)?;
                                let sink = crate::native_export::NativeFileExportSink { binding, directory:destination.clone() };
                                crate::resident_export::export_file(&mut owner.runtime.owner, &owner.runtime.storage, intent, &sink).await
                            }.await;
                            *lane_active.lock().expect("admission diagnostic mutex") = None;
                            let _ = reply.send(result.map_err(|error: anyhow::Error| format!("{error:#}")));
                        }
                        LaneRequest::Snapshot { expected, reply } => {
                            let _ = reply.send(owner.runtime.capture_fork_base(&expected).await.map_err(|error| error.to_string()));
                        }
                        LaneRequest::Catalog { reply } => {
                            let _ = reply.send(owner.runtime.catalog_descriptor().await.map_err(|error| error.to_string()));
                        }
                        LaneRequest::Diagnostics { reply } => {
                            let book = owner.runtime.owner.diagnostic_workbook();
                            let counters = book.evaluator_counters();
                            let _ = reply.send(serde_json::json!({"ingests":counters.ingests,"evaluations":counters.evaluations,"serializations":book.serialization_count()}));
                        }
                        LaneRequest::Detach { reply } => { drop(owner); let _ = reply.send(()); break; }
                    }
                }
            });
        })?;
        match ready_rx.await.context("owner lane exited during startup")? {
            Ok(response) => Ok((
                NativeOwnerLane {
                    tx,
                    active_request,
                    thread: Some(thread),
                },
                response,
            )),
            Err(error) => {
                let _ = thread.join();
                bail!(error)
            }
        }
    }
}

enum LaneRequest {
    Materialize {
        intent: crate::resident_export::FileExportIntent,
        reply: oneshot::Sender<Result<crate::resident_export::FileExportResult, String>>,
    },
    Snapshot {
        expected: String,
        reply: oneshot::Sender<std::result::Result<Vec<u8>, String>>,
    },
    Catalog {
        reply: oneshot::Sender<
            std::result::Result<
                Option<crate::canonical_lifecycle::CanonicalForkDescriptor>,
                String,
            >,
        >,
    },
    Execute {
        request_id: String,
        operation: String,
        payload: Value,
        verification_baseline: Option<(
            crate::operations::ResourceId,
            crate::canonical_lifecycle::VerificationSnapshot,
        )>,
        reply: oneshot::Sender<std::result::Result<CanonicalResponse, CanonicalErrorEnvelope>>,
    },
    Detach {
        reply: oneshot::Sender<()>,
    },
    Diagnostics {
        reply: oneshot::Sender<Value>,
    },
}

pub struct NativeOwnerLane {
    tx: mpsc::Sender<LaneRequest>,
    active_request: Arc<std::sync::Mutex<Option<String>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl NativeOwnerLane {
    pub async fn materialize(
        &self,
        intent: crate::resident_export::FileExportIntent,
    ) -> Result<crate::resident_export::FileExportResult> {
        let (reply, response) = oneshot::channel();
        let identity = intent.request_id.clone();
        self.tx
            .try_send(LaneRequest::Materialize { intent, reply })
            .map_err(|_| anyhow::anyhow!("owner queue unavailable; materialize not admitted"))?;
        response
            .await
            .with_context(|| format!("accepted materialize outcome unknown for {identity}"))?
            .map_err(anyhow::Error::msg)
    }

    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
    /// Ownership/admission metadata only; never queues a workbook read behind
    /// the operation being diagnosed, and never presents a workbook revision.
    pub fn admission(&self) -> Value {
        serde_json::json!({"active_request_id":*self.active_request.lock().expect("admission diagnostic mutex"),
            "queued_requests":LANE_QUEUE_CAPACITY - self.tx.capacity(), "closed":self.tx.is_closed()})
    }
    /// A full queue rejects before admission. After admission, channel loss is
    /// always uncertain; callers retain the request identity for reconciliation.
    pub async fn execute(
        &self,
        request_id: String,
        operation: String,
        payload: Value,
    ) -> Result<std::result::Result<CanonicalResponse, CanonicalErrorEnvelope>> {
        self.execute_with_verification_baseline(request_id, operation, payload, None)
            .await
    }
    pub async fn execute_with_verification_baseline(
        &self,
        request_id: String,
        operation: String,
        payload: Value,
        verification_baseline: Option<(
            crate::operations::ResourceId,
            crate::canonical_lifecycle::VerificationSnapshot,
        )>,
    ) -> Result<std::result::Result<CanonicalResponse, CanonicalErrorEnvelope>> {
        ensure!(
            !request_id.is_empty() && request_id.len() <= 256,
            "invalid request identity"
        );
        ensure!(
            serde_json::to_vec(&payload)?.len() <= MAX_REQUEST_BYTES,
            "request exceeds admission limit"
        );
        let (reply, response) = oneshot::channel();
        self.tx
            .try_send(LaneRequest::Execute {
                request_id: request_id.clone(),
                operation,
                payload,
                verification_baseline,
                reply,
            })
            .map_err(|_| anyhow::anyhow!("owner queue unavailable; request not admitted"))?;
        response.await.with_context(|| {
            format!("outcome unknown for accepted request {request_id}; recover and reconcile")
        })
    }
    pub async fn capture_fork_base(&self, expected: String) -> Result<Vec<u8>> {
        let (reply, response) = oneshot::channel();
        self.tx
            .try_send(LaneRequest::Snapshot { expected, reply })
            .context("owner queue unavailable")?;
        response
            .await
            .context("parent snapshot unavailable")?
            .map_err(anyhow::Error::msg)
    }
    pub async fn catalog_descriptor(
        &self,
    ) -> Result<Option<crate::canonical_lifecycle::CanonicalForkDescriptor>> {
        let (reply, response) = oneshot::channel();
        self.tx
            .try_send(LaneRequest::Catalog { reply })
            .context("owner queue unavailable")?;
        response
            .await
            .context("catalog owner exited")?
            .map_err(anyhow::Error::msg)
    }
    pub async fn diagnostics(&self) -> Result<Value> {
        let (reply, response) = oneshot::channel();
        self.tx
            .try_send(LaneRequest::Diagnostics { reply })
            .context("owner queue unavailable")?;
        Ok(response.await?)
    }
    pub async fn detach(mut self) -> Result<()> {
        let (reply, response) = oneshot::channel();
        if !self.tx.is_closed() {
            match self.tx.send(LaneRequest::Detach { reply }).await {
                Ok(()) => response
                    .await
                    .context("owner lane failed while detaching")?,
                Err(_) if self.tx.is_closed() => (), // Terminal lane is already draining.
                Err(error) => return Err(error).context("owner lane unavailable"),
            }
        }
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| anyhow::anyhow!("owner lane panicked"))?;
        }
        Ok(())
    }
}
