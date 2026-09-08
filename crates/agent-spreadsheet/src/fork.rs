use crate::security::canonicalize_and_enforce_within_workspace;
use crate::utils::make_short_random_id;
use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use web_time::Instant;

const FORK_DIR: &str = "/tmp/mcp-forks";
const CHECKPOINT_DIR: &str = "/tmp/mcp-checkpoints";
#[allow(dead_code)]
const STAGED_SNAPSHOT_DIR: &str = "/tmp/mcp-staged";
const DEFAULT_TTL_SECS: u64 = 0;
const DEFAULT_MAX_FORKS: usize = 10;
const CLEANUP_TASK_CHECK_SECS: u64 = 60;
const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024; // 100MB
const DEFAULT_MAX_CHECKPOINTS_PER_FORK: usize = 10;
const DEFAULT_MAX_STAGED_CHANGES_PER_FORK: usize = 20;
const DEFAULT_MAX_CHECKPOINT_TOTAL_BYTES: u64 = 500 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct EditOp {
    pub timestamp: DateTime<Utc>,
    pub sheet: String,
    pub address: String,
    pub value: String,
    pub is_formula: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StagedOp {
    pub kind: String,
    pub payload: JsonValue,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ChangeSummary {
    pub op_kinds: Vec<String>,
    pub affected_sheets: Vec<String>,
    pub affected_bounds: Vec<String>,
    pub counts: BTreeMap<String, u64>,
    #[serde(default)]
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub flags: BTreeMap<String, bool>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct StagedChange {
    pub change_id: String,
    pub created_at: DateTime<Utc>,
    pub label: Option<String>,
    pub ops: Vec<StagedOp>,
    pub summary: ChangeSummary,
    pub fork_path_snapshot: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct Checkpoint {
    pub checkpoint_id: String,
    pub created_at: DateTime<Utc>,
    pub label: Option<String>,
    pub snapshot_path: PathBuf,
    pub recalc_needed: bool,
    pub(crate) snapshot_state_revision: String,
    pub(crate) canonical_operation_len: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CanonicalOperationRecord {
    pub sequence: u64,
    pub timestamp: Option<String>,
    pub kind: String,
    pub op_kinds: Vec<String>,
    pub revision_before: String,
    pub revision_after: String,
}

#[derive(Debug)]
pub struct ForkContext {
    pub fork_id: String,
    pub base_path: PathBuf,
    pub work_path: PathBuf,
    pub created_at: Instant,
    pub last_accessed: Instant,
    pub edits: Vec<EditOp>,
    pub staged_changes: Vec<StagedChange>,
    pub checkpoints: Vec<Checkpoint>,
    pub(crate) canonical_operations: Vec<CanonicalOperationRecord>,
    pub recalc_needed: bool,
    pub(crate) state_revision: String,
    pub(crate) content_revision: String,
    discarded: bool,
    base_hash: String,
    base_modified: std::time::SystemTime,
}

impl ForkContext {
    fn new(fork_id: String, base_path: PathBuf, work_path: PathBuf) -> Result<Self> {
        let metadata = fs::metadata(&base_path)?;
        let base_modified = metadata.modified()?;
        let base_hash = hash_file(&base_path)?;
        let content_revision = hash_file(&work_path)?;

        Ok(Self {
            fork_id,
            base_path,
            work_path,
            created_at: Instant::now(),
            last_accessed: Instant::now(),
            edits: Vec::new(),
            staged_changes: Vec::new(),
            checkpoints: Vec::new(),
            canonical_operations: Vec::new(),
            recalc_needed: false,
            state_revision: content_revision.clone(),
            content_revision,
            discarded: false,
            base_hash,
            base_modified,
        })
    }

    pub fn is_expired(&self, ttl: Duration) -> bool {
        if ttl.is_zero() {
            return false;
        }
        self.last_accessed.elapsed() > ttl
    }

    pub fn touch(&mut self) {
        self.last_accessed = Instant::now();
    }

    pub(crate) fn sync_revisions(&mut self) -> Result<String> {
        let content_revision = hash_file(&self.work_path)?;
        if content_revision != self.content_revision {
            self.content_revision = content_revision.clone();
            self.state_revision = content_revision;
        }
        Ok(self.state_revision.clone())
    }

    pub(crate) fn advance_state_revision(&mut self) -> String {
        let revision = format!("state:{}", make_short_random_id("rev", 20));
        self.state_revision = revision.clone();
        revision
    }

    pub(crate) fn push_staged_change(&mut self, staged: StagedChange) {
        self.staged_changes.push(staged);
        enforce_staged_limits(self);
    }

    pub(crate) fn push_canonical_operation(
        &mut self,
        kind: impl Into<String>,
        op_kinds: Vec<String>,
        revision_before: String,
        revision_after: String,
    ) {
        self.canonical_operations.push(CanonicalOperationRecord {
            sequence: self
                .canonical_operations
                .last()
                .map_or(1, |operation| operation.sequence + 1),
            timestamp: Some(Utc::now().to_rfc3339()),
            kind: kind.into(),
            op_kinds,
            revision_before,
            revision_after,
        });
    }

    pub fn validate_base_unchanged(&self) -> Result<()> {
        let metadata = fs::metadata(&self.base_path)?;
        let current_modified = metadata.modified()?;

        if current_modified != self.base_modified {
            return Err(anyhow!("base file modified since fork creation"));
        }

        let current_hash = hash_file(&self.base_path)?;
        if current_hash != self.base_hash {
            return Err(anyhow!("base file content changed since fork creation"));
        }

        Ok(())
    }

    pub(crate) fn checkpoint_dir(&self) -> PathBuf {
        PathBuf::from(CHECKPOINT_DIR).join(&self.fork_id)
    }

    fn cleanup_files(&self) {
        let _ = fs::remove_file(&self.work_path);
        for staged in &self.staged_changes {
            remove_staged_snapshot(staged);
        }
        let checkpoint_dir = self.checkpoint_dir();
        if checkpoint_dir.starts_with(CHECKPOINT_DIR) {
            let _ = fs::remove_dir_all(&checkpoint_dir);
        }
    }
}

fn hash_file(path: &Path) -> Result<String> {
    let contents = fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&contents);
    Ok(format!("{:x}", hasher.finalize()))
}

#[derive(Debug, Clone)]
pub struct ForkConfig {
    pub ttl: Duration,
    pub max_forks: usize,
    pub fork_dir: PathBuf,
}

impl Default for ForkConfig {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(DEFAULT_TTL_SECS),
            max_forks: DEFAULT_MAX_FORKS,
            fork_dir: PathBuf::from(FORK_DIR),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ForkSnapshot {
    pub work_path: PathBuf,
    pub base_path: PathBuf,
    pub state_revision: String,
}

#[derive(Debug, Clone)]
pub struct ArtifactRecord {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

pub struct ForkRegistry {
    forks: Mutex<HashMap<String, Arc<Mutex<ForkContext>>>>,
    artifacts: Mutex<HashMap<String, ArtifactRecord>>,
    config: ForkConfig,
}

impl ForkRegistry {
    pub fn new(config: ForkConfig) -> Result<Self> {
        fs::create_dir_all(&config.fork_dir)?;
        Ok(Self {
            forks: Mutex::new(HashMap::new()),
            artifacts: Mutex::new(HashMap::new()),
            config,
        })
    }

    pub fn start_cleanup_task(self: Arc<Self>) {
        if self.config.ttl.is_zero() {
            return;
        }
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(CLEANUP_TASK_CHECK_SECS));
            loop {
                interval.tick().await;
                self.evict_expired();
            }
        });
    }

    pub fn create_fork(&self, base_path: &Path, workspace_root: &Path) -> Result<String> {
        self.evict_expired();

        {
            let forks = self.forks.lock();
            if forks.len() >= self.config.max_forks {
                return Err(anyhow!(
                    "max forks ({}) reached, discard existing forks first",
                    self.config.max_forks
                ));
            }
        }

        let ext = base_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase());

        if ext.as_deref() != Some("xlsx") {
            return Err(anyhow!(
                "only .xlsx files supported for fork/recalc (got {:?})",
                ext
            ));
        }

        if !base_path.exists() {
            return Err(anyhow!("base file does not exist: {:?}", base_path));
        }

        // Enforce workspace boundary using canonicalized, symlink-aware paths.
        let base_path_canon = canonicalize_and_enforce_within_workspace(
            workspace_root,
            base_path,
            "create_fork",
            "base_path",
        )?;

        let metadata = fs::metadata(&base_path_canon)?;
        if metadata.len() > MAX_FILE_SIZE {
            return Err(anyhow!(
                "base file too large: {} bytes (max {} MB)",
                metadata.len(),
                MAX_FILE_SIZE / 1024 / 1024
            ));
        }

        let fork_id = {
            let mut attempts: u32 = 0;
            loop {
                let candidate = make_short_random_id("fork", 12);
                let work_path = self.config.fork_dir.join(format!("{}.xlsx", candidate));
                let exists_in_registry = self.forks.lock().contains_key(&candidate);
                if !exists_in_registry && !work_path.exists() {
                    break candidate;
                }
                attempts += 1;
                if attempts > 20 {
                    return Err(anyhow!("failed to allocate unique fork id"));
                }
            }
        };
        let work_path = self.config.fork_dir.join(format!("{}.xlsx", fork_id));

        fs::copy(&base_path_canon, &work_path)?;

        let context = ForkContext::new(fork_id.clone(), base_path_canon, work_path.clone())?;

        let inserted = {
            let mut forks = self.forks.lock();
            if forks.len() >= self.config.max_forks {
                false
            } else {
                forks.insert(fork_id.clone(), Arc::new(Mutex::new(context)));
                true
            }
        };
        if !inserted {
            let _ = fs::remove_file(&work_path);
            return Err(anyhow!(
                "max forks ({}) reached, discard existing forks first",
                self.config.max_forks
            ));
        }
        Ok(fork_id)
    }

    fn fork_handle(&self, fork_id: &str) -> Result<Arc<Mutex<ForkContext>>> {
        self.forks
            .lock()
            .get(fork_id)
            .cloned()
            .ok_or_else(|| anyhow!("fork not found: {}", fork_id))
    }

    pub fn get_fork(&self, fork_id: &str) -> Result<Arc<ForkContext>> {
        self.evict_expired();
        let handle = self.fork_handle(fork_id)?;
        let mut ctx = handle.lock();
        if ctx.discarded {
            return Err(anyhow!("fork not found: {}", fork_id));
        }
        ctx.touch();
        Ok(Arc::new(ctx.clone()))
    }

    pub fn get_fork_path(&self, fork_id: &str) -> Option<PathBuf> {
        let handle = self.fork_handle(fork_id).ok()?;
        let mut ctx = handle.lock();
        if ctx.discarded {
            return None;
        }
        ctx.touch();
        Some(ctx.work_path.clone())
    }

    pub fn with_fork_mut<F, R>(&self, fork_id: &str, f: F) -> Result<R>
    where
        F: FnOnce(&mut ForkContext) -> Result<R>,
    {
        let handle = self.fork_handle(fork_id)?;
        let mut ctx = handle.lock();
        if ctx.discarded {
            return Err(anyhow!("fork not found: {}", fork_id));
        }
        ctx.touch();
        f(&mut ctx)
    }

    pub fn discard_fork(&self, fork_id: &str) -> Result<()> {
        let handle = match self.fork_handle(fork_id) {
            Ok(handle) => handle,
            Err(_) => return Ok(()),
        };
        let mut ctx = handle.lock();
        ctx.discarded = true;
        self.forks.lock().remove(fork_id);
        ctx.cleanup_files();
        Ok(())
    }

    pub(crate) fn discard_fork_cas(
        &self,
        fork_id: &str,
        expected_revision: &str,
    ) -> Result<(String, PathBuf)> {
        let handle = self.fork_handle(fork_id)?;
        let mut ctx = handle.lock();
        if ctx.discarded {
            return Err(anyhow!("fork not found: {}", fork_id));
        }
        let current = ctx.sync_revisions()?;
        if current != expected_revision {
            return Err(anyhow!(
                "revision conflict: expected {}, current {}",
                expected_revision,
                current
            ));
        }
        let revision_before = ctx.state_revision.clone();
        let work_path = ctx.work_path.clone();
        ctx.discarded = true;
        self.forks.lock().remove(fork_id);
        ctx.cleanup_files();
        Ok((revision_before, work_path))
    }

    pub fn save_fork(
        &self,
        fork_id: &str,
        target_path: &Path,
        workspace_root: &Path,
        drop_fork: bool,
    ) -> Result<()> {
        // Enforce workspace boundary using canonicalized, symlink-aware paths.
        let _target_canon = canonicalize_and_enforce_within_workspace(
            workspace_root,
            target_path,
            "save_fork",
            "target_path",
        )?;

        let ext = target_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase());

        if ext.as_deref() != Some("xlsx") {
            return Err(anyhow!("target must be .xlsx"));
        }

        self.with_fork_mut(fork_id, |ctx| {
            ctx.validate_base_unchanged()?;
            fs::copy(&ctx.work_path, target_path)?;
            Ok(())
        })?;
        if drop_fork {
            self.discard_fork(fork_id)?;
        }
        Ok(())
    }

    pub fn ttl(&self) -> Duration {
        self.config.ttl
    }

    pub fn list_forks(&self) -> Vec<ForkInfo> {
        self.evict_expired();

        let handles = self.forks.lock().values().cloned().collect::<Vec<_>>();
        handles
            .into_iter()
            .filter_map(|handle| {
                let ctx = handle.lock();
                (!ctx.discarded).then(|| ForkInfo {
                    fork_id: ctx.fork_id.clone(),
                    base_path: ctx.base_path.display().to_string(),
                    created_at: ctx.created_at,
                    edit_count: ctx.edits.len(),
                    recalc_needed: ctx.recalc_needed,
                })
            })
            .collect()
    }

    pub fn create_checkpoint(&self, fork_id: &str, label: Option<String>) -> Result<Checkpoint> {
        self.evict_expired();

        self.with_fork_mut(fork_id, |ctx| {
            ctx.sync_revisions()?;
            let checkpoint_id = make_short_random_id("cp", 12);
            let dir = PathBuf::from(CHECKPOINT_DIR).join(fork_id);
            fs::create_dir_all(&dir)?;
            let snapshot_path = dir.join(format!("{}.xlsx", checkpoint_id));
            fs::copy(&ctx.work_path, &snapshot_path)?;
            let checkpoint = Checkpoint {
                checkpoint_id,
                created_at: Utc::now(),
                label,
                snapshot_path,
                recalc_needed: ctx.recalc_needed,
                snapshot_state_revision: ctx.state_revision.clone(),
                canonical_operation_len: ctx.canonical_operations.len(),
            };
            ctx.checkpoints.push(checkpoint.clone());
            enforce_checkpoint_limits(ctx)?;
            ctx.advance_state_revision();
            Ok(checkpoint)
        })
    }

    pub fn list_checkpoints(&self, fork_id: &str) -> Result<Vec<Checkpoint>> {
        let ctx = self.get_fork(fork_id)?;
        Ok(ctx.checkpoints.clone())
    }

    pub fn delete_checkpoint(&self, fork_id: &str, checkpoint_id: &str) -> Result<()> {
        self.with_fork_mut(fork_id, |ctx| {
            let index = ctx
                .checkpoints
                .iter()
                .position(|c| c.checkpoint_id == checkpoint_id)
                .ok_or_else(|| anyhow!("checkpoint not found: {}", checkpoint_id))?;
            let removed = ctx.checkpoints.remove(index);
            let _ = fs::remove_file(&removed.snapshot_path);
            ctx.advance_state_revision();
            Ok(())
        })
    }

    pub fn restore_checkpoint(&self, fork_id: &str, checkpoint_id: &str) -> Result<Checkpoint> {
        self.evict_expired();

        let checkpoint = self.with_fork_mut(fork_id, |ctx| {
            let checkpoint = ctx
                .checkpoints
                .iter()
                .find(|c| c.checkpoint_id == checkpoint_id)
                .cloned()
                .ok_or_else(|| anyhow!("checkpoint not found: {}", checkpoint_id))?;
            fs::copy(&checkpoint.snapshot_path, &ctx.work_path)?;
            let cutoff = checkpoint.created_at;
            ctx.edits.retain(|e| e.timestamp <= cutoff);
            let mut i = 0;
            while i < ctx.staged_changes.len() {
                if ctx.staged_changes[i].created_at > cutoff {
                    let removed = ctx.staged_changes.remove(i);
                    remove_staged_snapshot(&removed);
                } else {
                    i += 1;
                }
            }
            ctx.recalc_needed = checkpoint.recalc_needed;
            ctx.content_revision = hash_file(&ctx.work_path)?;
            ctx.advance_state_revision();
            Ok(checkpoint)
        })?;

        Ok(checkpoint)
    }

    pub fn add_staged_change(&self, fork_id: &str, staged: StagedChange) -> Result<()> {
        self.with_fork_mut(fork_id, |ctx| {
            ctx.push_staged_change(staged);
            ctx.advance_state_revision();
            Ok(())
        })
    }

    pub fn list_staged_changes(&self, fork_id: &str) -> Result<Vec<StagedChange>> {
        let ctx = self.get_fork(fork_id)?;
        Ok(ctx.staged_changes.clone())
    }

    pub fn take_staged_change(&self, fork_id: &str, change_id: &str) -> Result<StagedChange> {
        self.with_fork_mut(fork_id, |ctx| {
            let index = ctx
                .staged_changes
                .iter()
                .position(|c| c.change_id == change_id)
                .ok_or_else(|| anyhow!("staged change not found: {}", change_id))?;
            let staged = ctx.staged_changes.remove(index);
            ctx.advance_state_revision();
            Ok(staged)
        })
    }

    pub fn discard_staged_change(&self, fork_id: &str, change_id: &str) -> Result<()> {
        let removed = self.take_staged_change(fork_id, change_id)?;
        remove_staged_snapshot(&removed);
        Ok(())
    }

    pub(crate) fn sync_fork_revisions(&self, fork_id: &str) -> Result<(String, String)> {
        self.with_fork_mut(fork_id, |fork| {
            let state_revision = fork.sync_revisions()?;
            Ok((state_revision, fork.content_revision.clone()))
        })
    }

    pub(crate) fn snapshot_forks(
        &self,
        fork_ids: &[String],
        destination_dir: &Path,
    ) -> Result<Vec<ForkSnapshot>> {
        let mut unique_ids = fork_ids.to_vec();
        unique_ids.sort();
        unique_ids.dedup();
        let handles = {
            let forks = self.forks.lock();
            unique_ids
                .iter()
                .map(|id| {
                    forks
                        .get(id)
                        .cloned()
                        .map(|handle| (id.clone(), handle))
                        .ok_or_else(|| anyhow!("fork not found: {}", id))
                })
                .collect::<Result<Vec<_>>>()?
        };
        let mut guards = handles
            .iter()
            .map(|(_, handle)| handle.lock())
            .collect::<Vec<_>>();
        for (index, guard) in guards.iter_mut().enumerate() {
            if guard.discarded {
                return Err(anyhow!("fork not found: {}", unique_ids[index]));
            }
            guard.touch();
            guard.sync_revisions()?;
        }

        fork_ids
            .iter()
            .enumerate()
            .map(|(index, id)| {
                let position = unique_ids
                    .binary_search(id)
                    .expect("requested fork id was locked");
                let guard = &guards[position];
                let work_path = destination_dir.join(format!("fork-{index}-work.xlsx"));
                let base_path = destination_dir.join(format!("fork-{index}-base.xlsx"));
                fs::copy(&guard.work_path, &work_path)?;
                fs::copy(&guard.base_path, &base_path)?;
                Ok(ForkSnapshot {
                    work_path,
                    base_path,
                    state_revision: guard.state_revision.clone(),
                })
            })
            .collect()
    }

    pub(crate) fn register_artifact(&self, artifact_id: String, record: ArtifactRecord) {
        self.artifacts.lock().insert(artifact_id, record);
    }

    pub fn resolve_artifact(&self, artifact_id: &str) -> Option<ArtifactRecord> {
        self.artifacts.lock().get(artifact_id).cloned()
    }

    fn evict_expired(&self) {
        if self.config.ttl.is_zero() {
            return;
        }
        let handles = self
            .forks
            .lock()
            .iter()
            .map(|(id, handle)| (id.clone(), handle.clone()))
            .collect::<Vec<_>>();
        for (id, handle) in handles {
            let mut ctx = handle.lock();
            if ctx.discarded || !ctx.is_expired(self.config.ttl) {
                continue;
            }
            ctx.discarded = true;
            self.forks.lock().remove(&id);
            ctx.cleanup_files();
            tracing::debug!(fork_id = %id, "evicted expired fork");
        }
    }
}

pub(crate) fn remove_staged_snapshot(staged: &StagedChange) {
    if let Some(path) = staged.fork_path_snapshot.as_ref() {
        let _ = fs::remove_file(path);
    }
}

fn enforce_staged_limits(ctx: &mut ForkContext) {
    while ctx.staged_changes.len() > DEFAULT_MAX_STAGED_CHANGES_PER_FORK {
        let removed = ctx.staged_changes.remove(0);
        remove_staged_snapshot(&removed);
    }
}

pub(crate) fn enforce_checkpoint_limits(ctx: &mut ForkContext) -> Result<()> {
    while ctx.checkpoints.len() > DEFAULT_MAX_CHECKPOINTS_PER_FORK {
        let removed = ctx.checkpoints.remove(0);
        let _ = fs::remove_file(&removed.snapshot_path);
    }

    loop {
        let mut total_bytes = 0u64;
        for cp in &ctx.checkpoints {
            if let Ok(meta) = fs::metadata(&cp.snapshot_path) {
                total_bytes += meta.len();
            }
        }
        if total_bytes <= DEFAULT_MAX_CHECKPOINT_TOTAL_BYTES || ctx.checkpoints.len() <= 1 {
            break;
        }
        let removed = ctx.checkpoints.remove(0);
        let _ = fs::remove_file(&removed.snapshot_path);
    }

    Ok(())
}

impl Clone for ForkContext {
    fn clone(&self) -> Self {
        Self {
            fork_id: self.fork_id.clone(),
            base_path: self.base_path.clone(),
            work_path: self.work_path.clone(),
            created_at: self.created_at,
            last_accessed: self.last_accessed,
            edits: self.edits.clone(),
            staged_changes: self.staged_changes.clone(),
            checkpoints: self.checkpoints.clone(),
            canonical_operations: self.canonical_operations.clone(),
            recalc_needed: self.recalc_needed,
            state_revision: self.state_revision.clone(),
            content_revision: self.content_revision.clone(),
            discarded: self.discarded,
            base_hash: self.base_hash.clone(),
            base_modified: self.base_modified,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ForkInfo {
    pub fork_id: String,
    pub base_path: String,
    pub created_at: Instant,
    pub edit_count: usize,
    pub recalc_needed: bool,
}
