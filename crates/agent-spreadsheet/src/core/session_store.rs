//! Persistent session storage backed by project-local `.asp/` directories.
//!
//! Layout:
//! ```text
//! .asp/
//!   sessions/
//!     <session_id>/
//!       base.xlsx                 # Immutable base file
//!       events.jsonl              # Append-only OpEvent log
//!       HEAD                      # Active op_id
//!       CURRENT_BRANCH            # Branch name pointer
//!       branches.json             # Branch metadata
//!       staged/
//!         <staged_id>.json        # Staged op payloads + computed impact
//!       snapshots/
//!         manifest.json           # Snapshot index
//!         <op_id>.xlsx            # Materialized snapshot files
//!       locks/
//!         session.lock            # Exclusive apply lock
//! ```

use crate::core::binlog::{
    BinlogReader, BinlogWriter, BranchInfo, BranchesFile, SnapshotEntry, SnapshotManifest,
};
use crate::core::events::OpEvent;
use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

/// Default snapshot interval (create a snapshot every N events).
const SNAPSHOT_INTERVAL: usize = 10;

/// Maximum number of concurrent sessions.
const MAX_SESSIONS: usize = 50;

fn validate_session_id(session_id: &str) -> Result<()> {
    if session_id.is_empty()
        || !session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        bail!("invalid session id");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// SessionStore
// ---------------------------------------------------------------------------

/// Persistent session store managing `.asp/sessions/` directories.
pub struct SessionStore {
    root: PathBuf,
}

impl SessionStore {
    /// Open or create a session store at the given workspace root.
    /// The `.asp/sessions/` directory is created if it doesn't exist.
    pub fn open(workspace_root: impl Into<PathBuf>) -> Result<Self> {
        let root = workspace_root.into().join(".asp").join("sessions");
        fs::create_dir_all(&root)
            .with_context(|| format!("failed to create session store: {}", root.display()))?;
        Ok(Self { root })
    }

    /// Create a new session from a base workbook file.
    pub fn create_session(&self, base_path: &Path, label: Option<&str>) -> Result<SessionHandle> {
        let session_count = self.list_sessions()?.len();
        if session_count >= MAX_SESSIONS {
            bail!(
                "session limit reached: {} sessions exist (max {})",
                session_count,
                MAX_SESSIONS
            );
        }

        if !base_path.exists() {
            bail!("base file not found: {}", base_path.display());
        }

        let session_id = make_session_id();
        let session_dir = self.root.join(&session_id);
        fs::create_dir_all(&session_dir)?;

        // Copy base file (immutable)
        let base_dest = session_dir.join("base.xlsx");
        fs::copy(base_path, &base_dest).with_context(|| {
            format!(
                "failed to copy base file from '{}' to '{}'",
                base_path.display(),
                base_dest.display()
            )
        })?;

        // Initialize empty binlog
        let binlog_path = session_dir.join("events.jsonl");
        fs::write(&binlog_path, "")?;

        // Initialize HEAD (empty = at base, no events applied)
        fs::write(session_dir.join("HEAD"), "")?;

        // Initialize branch pointer
        fs::write(session_dir.join("CURRENT_BRANCH"), "main")?;

        // Initialize branches.json
        let branches = BranchesFile::new();
        branches.save(&session_dir.join("branches.json"))?;

        // Initialize snapshot manifest
        let manifest = SnapshotManifest::new(session_id.clone());
        let snapshots_dir = session_dir.join("snapshots");
        fs::create_dir_all(&snapshots_dir)?;
        manifest.save(&snapshots_dir.join("manifest.json"))?;

        // Create staged and locks directories
        fs::create_dir_all(session_dir.join("staged"))?;
        fs::create_dir_all(session_dir.join("locks"))?;

        // Write session metadata
        let meta = SessionMeta {
            session_id: session_id.clone(),
            label: label.map(|s| s.to_string()),
            base_path: base_path.to_path_buf(),
            created_at: chrono::Utc::now(),
        };
        let meta_json = serde_json::to_string_pretty(&meta)?;
        fs::write(session_dir.join("session.json"), meta_json)?;

        SessionHandle::open(&self.root, &session_id)
    }

    /// Open an existing session by ID.
    pub fn open_session(&self, session_id: &str) -> Result<SessionHandle> {
        validate_session_id(session_id)?;
        let handle = SessionHandle::open(&self.root, session_id)?;
        handle.ensure_legacy_writable()?;
        Ok(handle)
    }

    /// Import-only access to an explicitly frozen copy. Ordinary legacy tools
    /// cannot open this handle through their public store entrypoint.
    pub(crate) fn open_frozen_import(&self, session_id: &str) -> Result<SessionHandle> {
        validate_session_id(session_id)?;
        let handle = SessionHandle::open(&self.root, session_id)?;
        if !handle
            .dir
            .join("resident-import-frozen.json")
            .try_exists()?
        {
            bail!("legacy import requires an explicitly frozen copy");
        }
        Ok(handle)
    }

    /// List all session IDs.
    pub fn list_sessions(&self) -> Result<Vec<String>> {
        let mut sessions = Vec::new();
        if self.root.exists() {
            for entry in fs::read_dir(&self.root)? {
                let entry = entry?;
                if entry.file_type()?.is_dir()
                    && let Some(name) = entry.file_name().to_str()
                    && name.starts_with("sess_")
                {
                    sessions.push(name.to_string());
                }
            }
        }
        sessions.sort();
        Ok(sessions)
    }

    /// Delete a session and all its artifacts.
    pub fn delete_session(&self, session_id: &str) -> Result<()> {
        validate_session_id(session_id)?;
        let session_dir = self.root.join(session_id);
        if !session_dir.exists() {
            bail!("session not found: {}", session_id);
        }
        if session_dir
            .join("resident-import-frozen.json")
            .try_exists()?
        {
            bail!("legacy session is frozen for resident import; deletion refused");
        }
        fs::remove_dir_all(&session_dir)
            .with_context(|| format!("failed to delete session: {}", session_id))
    }
}

// ---------------------------------------------------------------------------
// SessionMeta
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionMeta {
    pub session_id: String,
    pub label: Option<String>,
    pub base_path: PathBuf,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

// ---------------------------------------------------------------------------
// SessionHandle
// ---------------------------------------------------------------------------

/// Handle for interacting with a single persistent session.
pub struct SessionHandle {
    pub session_id: String,
    dir: PathBuf,
}

impl SessionHandle {
    fn open(store_root: &Path, session_id: &str) -> Result<Self> {
        let dir = store_root.join(session_id);
        if !dir.exists() {
            bail!("session not found: {}", session_id);
        }
        Ok(Self {
            session_id: session_id.to_string(),
            dir,
        })
    }

    // -- Paths --

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn base_path(&self) -> PathBuf {
        self.dir.join("base.xlsx")
    }

    pub fn binlog_path(&self) -> PathBuf {
        self.dir.join("events.jsonl")
    }

    pub fn head_path(&self) -> PathBuf {
        self.dir.join("HEAD")
    }

    pub fn current_branch_path(&self) -> PathBuf {
        self.dir.join("CURRENT_BRANCH")
    }

    pub fn branches_path(&self) -> PathBuf {
        self.dir.join("branches.json")
    }

    pub fn snapshot_manifest_path(&self) -> PathBuf {
        self.dir.join("snapshots").join("manifest.json")
    }

    pub fn snapshot_file_path(&self, op_id: &str) -> PathBuf {
        self.dir.join("snapshots").join(format!("{}.xlsx", op_id))
    }

    pub fn staged_dir(&self) -> PathBuf {
        self.dir.join("staged")
    }

    pub fn lock_path(&self) -> PathBuf {
        self.dir.join("locks").join("session.lock")
    }

    // -- HEAD management --

    /// Read the current HEAD op_id. Returns None if HEAD is empty (at base).
    pub fn read_head(&self) -> Result<Option<String>> {
        let content = fs::read_to_string(self.head_path()).context("failed to read HEAD")?;
        let trimmed = content.trim();
        if trimmed.is_empty() {
            Ok(None)
        } else {
            Ok(Some(trimmed.to_string()))
        }
    }

    /// Set the HEAD op_id.
    pub fn write_head(&self, op_id: &str) -> Result<()> {
        self.ensure_legacy_writable()?;
        fs::write(self.head_path(), op_id).context("failed to write HEAD")
    }

    /// Clear HEAD back to base state.
    pub fn clear_head(&self) -> Result<()> {
        self.ensure_legacy_writable()?;
        fs::write(self.head_path(), "").context("failed to clear HEAD")
    }

    // -- Branch management --

    /// Read the current branch name.
    pub fn current_branch(&self) -> Result<String> {
        let content = fs::read_to_string(self.current_branch_path())
            .context("failed to read CURRENT_BRANCH")?;
        Ok(content.trim().to_string())
    }

    /// Switch to a different branch.
    pub fn switch_branch(&self, branch_name: &str) -> Result<()> {
        self.ensure_legacy_writable()?;
        let branches = BranchesFile::load(&self.branches_path())?;
        if branches.get_branch(branch_name).is_none() {
            bail!("branch not found: {}", branch_name);
        }

        fs::write(self.current_branch_path(), branch_name)
            .context("failed to write CURRENT_BRANCH")?;

        // Update HEAD to the branch tip
        let tip = branches
            .get_branch(branch_name)
            .and_then(|b| b.tip_op_id.clone());
        match tip {
            Some(op_id) => self.write_head(&op_id),
            None => self.clear_head(),
        }
    }

    /// Create a new branch forking from the given op_id.
    pub fn create_branch(
        &self,
        name: &str,
        fork_point: Option<&str>,
        label: Option<&str>,
    ) -> Result<()> {
        self.ensure_legacy_writable()?;
        let mut branches = BranchesFile::load(&self.branches_path())?;
        if branches.get_branch(name).is_some() {
            bail!("branch already exists: {}", name);
        }

        branches.add_branch(BranchInfo {
            name: name.to_string(),
            tip_op_id: fork_point.map(|s| s.to_string()),
            fork_point: fork_point.map(|s| s.to_string()),
            label: label.map(|s| s.to_string()),
            created_at: chrono::Utc::now(),
        });

        branches.save(&self.branches_path())
    }

    /// List all branches.
    pub fn list_branches(&self) -> Result<Vec<BranchInfo>> {
        let branches = BranchesFile::load(&self.branches_path())?;
        Ok(branches.branches)
    }

    // -- Event log --

    /// Append an event to the binlog and advance HEAD + branch tip.
    /// This is the atomic apply operation.
    pub fn append_event(&self, mut event: OpEvent) -> Result<()> {
        validate_session_id(&event.op_id)?;
        if event.session_id != self.session_id {
            bail!("event belongs to a different session");
        }
        // Acquire exclusive lock
        let _lock = self.acquire_lock()?;

        // Validate HEAD matches expected parent
        let current_head = self.read_head()?;
        if event.parent_id != current_head {
            bail!(
                "CAS conflict: event parent_id is {:?} but HEAD is {:?}",
                event.parent_id,
                current_head
            );
        }

        // Evaluate preconditions (cell_matches + workbook_hash_before)
        if let Some(ref preconditions) = event.preconditions {
            let has_cell_matches = !preconditions.cell_matches.is_empty();
            let has_hash_before = preconditions.workbook_hash_before.is_some();

            if has_cell_matches || has_hash_before {
                let wb_bytes = self.materialize()?;

                if has_hash_before {
                    let actual = compute_workbook_hash(&wb_bytes);
                    let expected = preconditions.workbook_hash_before.as_ref().unwrap();
                    if &actual != expected {
                        bail!(
                            "precondition failed: workbook_hash_before mismatch (expected {}, got {})",
                            expected,
                            actual
                        );
                    }
                }

                if has_cell_matches {
                    let ws = crate::core::session::WorkbookSession::from_bytes(&wb_bytes)?;
                    let violations = evaluate_cell_matches(&ws, &preconditions.cell_matches)?;
                    if !violations.is_empty() {
                        bail!("precondition failed: {}", violations.join("; "));
                    }
                }
            }
        }

        // Seal the event hash
        let prev_hash = if current_head.is_some() {
            let reader = BinlogReader::open(self.binlog_path())?;
            reader.tip()?.and_then(|e| e.event_hash)
        } else {
            None
        };
        event.prev_event_hash = prev_hash;
        event.seal();

        // Append to binlog
        let writer = BinlogWriter::open(self.binlog_path())?;
        writer.append(&event)?;

        // Advance HEAD
        self.write_head(&event.op_id)?;

        // Update branch tip
        let branch_name = self.current_branch()?;
        let mut branches = BranchesFile::load(&self.branches_path())?;
        if let Some(branch) = branches.get_branch_mut(&branch_name) {
            branch.tip_op_id = Some(event.op_id.clone());
        }
        branches.save(&self.branches_path())?;

        // Check if we should create a snapshot
        let reader = BinlogReader::open(self.binlog_path())?;
        let event_count = reader.count()?;
        if event_count > 0 && event_count % SNAPSHOT_INTERVAL == 0 {
            // Snapshot creation is best-effort; don't fail the append
            let _ = self.create_snapshot(&event.op_id, event_count);
        }

        Ok(())
    }

    /// Read all events in the binlog.
    pub fn read_events(&self) -> Result<Vec<OpEvent>> {
        let reader = BinlogReader::open(self.binlog_path())?;
        let events = reader.read_all()?;
        let mut by_id = std::collections::BTreeMap::new();
        let mut previous_hash = None;
        for event in &events {
            validate_session_id(&event.op_id)?;
            if event.session_id != self.session_id || !event.verify_integrity() {
                bail!(
                    "legacy event '{}' failed session/payload/hash validation",
                    event.op_id
                );
            }
            if event.prev_event_hash != previous_hash {
                bail!(
                    "legacy event append hash chain is corrupt at '{}'",
                    event.op_id
                );
            }
            previous_hash = event.event_hash.clone();
            if by_id.insert(event.op_id.clone(), event).is_some() {
                bail!("duplicate legacy event id '{}'", event.op_id);
            }
        }
        for event in &events {
            if let Some(parent) = &event.parent_id
                && !by_id.contains_key(parent)
            {
                bail!("legacy event '{}' references missing parent", event.op_id);
            }
        }
        let mut tips = BranchesFile::load(&self.branches_path())?
            .branches
            .into_iter()
            .filter_map(|branch| branch.tip_op_id)
            .collect::<Vec<_>>();
        tips.extend(self.read_head()?);
        let mut committed = std::collections::BTreeSet::new();
        for tip in tips {
            let mut cursor = Some(tip);
            while let Some(id) = cursor {
                if !committed.insert(id.clone()) {
                    break;
                }
                let event = by_id
                    .get(&id)
                    .ok_or_else(|| anyhow!("legacy projection references missing event '{id}'"))?;
                cursor = event.parent_id.clone();
            }
        }
        Ok(events
            .into_iter()
            .filter(|event| committed.contains(&event.op_id))
            .collect())
    }

    /// Read events after a given op_id.
    pub fn read_events_after(&self, after_op_id: &str) -> Result<Vec<OpEvent>> {
        let reader = BinlogReader::open(self.binlog_path())?;
        reader.read_after(after_op_id)
    }

    /// Get the event log for display (session log).
    pub fn log(&self) -> Result<Vec<OpEvent>> {
        self.read_events()
    }

    // -- Undo / Redo --

    /// Move HEAD back one event (branch-local undo).
    pub fn undo(&self) -> Result<Option<String>> {
        let head = self.read_head()?;
        let Some(head_id) = head else {
            bail!("already at base: nothing to undo");
        };

        let events = self.read_events()?;
        let head_event = events
            .iter()
            .find(|e| e.op_id == head_id)
            .ok_or_else(|| anyhow!("HEAD event '{}' not found in binlog", head_id))?;

        match &head_event.parent_id {
            Some(parent) => {
                self.write_head(parent)?;
                Ok(Some(parent.clone()))
            }
            None => {
                self.clear_head()?;
                Ok(None)
            }
        }
    }

    /// Move HEAD forward one event (branch-local redo).
    pub fn redo(&self) -> Result<Option<String>> {
        let head = self.read_head()?;
        let events = self.read_events()?;

        let next = match &head {
            Some(head_id) => events
                .iter()
                .find(|e| e.parent_id.as_deref() == Some(head_id)),
            None => events.first(),
        };

        match next {
            Some(event) => {
                self.write_head(&event.op_id)?;
                Ok(Some(event.op_id.clone()))
            }
            None => bail!("nothing to redo"),
        }
    }

    /// Set HEAD to a specific op_id (checkout).
    pub fn checkout(&self, op_id: &str) -> Result<()> {
        let events = self.read_events()?;
        if !events.iter().any(|e| e.op_id == op_id) {
            bail!("op_id '{}' not found in event log", op_id);
        }
        self.write_head(op_id)
    }

    // -- Snapshots --

    /// Create a snapshot at the given op_id.
    fn create_snapshot(&self, op_id: &str, event_count: usize) -> Result<()> {
        // Materialize the workbook at this point
        let materialized_bytes = self.materialize_at(op_id)?;
        let snapshot_path = self.snapshot_file_path(op_id);
        fs::write(&snapshot_path, &materialized_bytes)?;

        let hash = {
            let digest = Sha256::digest(&materialized_bytes);
            format!("sha256:{:x}", digest)
        };

        let mut manifest = SnapshotManifest::load(&self.snapshot_manifest_path())
            .unwrap_or_else(|_| SnapshotManifest::new(self.session_id.clone()));

        manifest.add_entry(SnapshotEntry {
            op_id: op_id.to_string(),
            file_name: format!("{}.xlsx", op_id),
            file_hash: hash,
            created_at: chrono::Utc::now(),
            event_count,
        });

        manifest.save(&self.snapshot_manifest_path())
    }

    // -- Materialization --

    /// Materialize the workbook at the current HEAD by loading base + replaying events.
    pub fn materialize(&self) -> Result<Vec<u8>> {
        let head = self.read_head()?;
        match head {
            Some(op_id) => self.materialize_at(&op_id),
            None => {
                // At base: just return the base file
                fs::read(self.base_path())
                    .context("failed to read base workbook for materialization")
            }
        }
    }

    /// Materialize the workbook at a specific op_id.
    pub fn materialize_at(&self, target_op_id: &str) -> Result<Vec<u8>> {
        let events = self.read_events()?;
        let by_id = events
            .iter()
            .map(|event| (event.op_id.as_str(), event))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut ancestry = Vec::new();
        let mut cursor = Some(target_op_id.to_string());
        let mut seen = std::collections::BTreeSet::new();
        while let Some(id) = cursor {
            if !seen.insert(id.clone()) {
                bail!("legacy event ancestry cycle");
            }
            let event = by_id
                .get(id.as_str())
                .ok_or_else(|| anyhow!("target event is absent from committed legacy history"))?;
            ancestry.push(*event);
            cursor = event.parent_id.clone();
        }
        ancestry.reverse();
        // Legacy snapshots are derived and untrusted; migration replays the
        // validated parent ancestry from the immutable base.
        let bytes = fs::read(self.base_path()).context("failed to read base workbook")?;
        let mut session = crate::core::session::WorkbookSession::from_bytes(&bytes)?;
        for event in ancestry {
            replay_event_on_session(&mut session, event)?;
        }
        session.to_bytes()
    }

    // -- Locking --

    fn ensure_legacy_writable(&self) -> Result<()> {
        if !fs::symlink_metadata(&self.dir)?.file_type().is_dir() { bail!("legacy session path is fenced or invalid"); }
        if self.dir.join("resident-import-frozen.json").try_exists()? {
            bail!("legacy session is frozen for resident import; mutation refused");
        }
        Ok(())
    }

    fn acquire_lock(&self) -> Result<SessionLock> {
        self.ensure_legacy_writable()?;
        use fs2::FileExt;
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(self.lock_path())?;
        if !file.metadata()?.is_file() {
            bail!("legacy lock must be a regular file");
        }
        file.lock_exclusive()?;
        self.ensure_legacy_writable()?;
        Ok(SessionLock { _file: file })
    }

    /// Read session metadata.
    pub fn meta(&self) -> Result<SessionMeta> {
        let content = fs::read_to_string(self.dir.join("session.json"))
            .context("failed to read session.json")?;
        serde_json::from_str(&content).context("failed to parse session.json")
    }
}

/// Closing the file releases OS ownership. Never unlink a lock inode: waiting
/// writers may already hold descriptors referring to it.
struct SessionLock {
    _file: fs::File,
}

// ---------------------------------------------------------------------------
// Event replay
// ---------------------------------------------------------------------------

/// Write session state to a temp file, apply a mutation via the provided
/// closure, then reload the session from the mutated file.
///
/// This reuses the battle-tested file-based `apply_*_to_file()` functions
/// and defers in-memory optimization to a future phase.
#[cfg(feature = "recalc")]
fn replay_via_temp_file<F>(
    session: &mut crate::core::session::WorkbookSession,
    apply_fn: F,
) -> Result<()>
where
    F: FnOnce(&Path) -> Result<()>,
{
    let tmp = session.to_temp_file()?;
    apply_fn(tmp.path())?;
    session.reload_from_path(tmp.path())?;
    Ok(())
}

/// Replay a single OpEvent on a WorkbookSession.
///
/// This is the core event-to-mutation mapping. Each OpKind is routed to the
/// appropriate session method or file-based apply function (via temp-file
/// round-trip).
#[cfg(feature = "recalc")]
fn replay_event_on_session(
    session: &mut crate::core::session::WorkbookSession,
    event: &OpEvent,
) -> Result<()> {
    use crate::core::session::SessionTransformOp;
    use crate::model::diagnostics::FormulaParsePolicy;
    use crate::tools::fork::{
        ApplyFormulaPatternOpInput, ColumnSizeOp, ReplaceInFormulasOp, StructureOp, StyleOp,
        TransformOp, apply_column_size_ops_to_file, apply_formula_pattern_ops_to_file,
        apply_replace_in_formulas_to_file, apply_structure_ops_to_file, apply_style_ops_to_file,
        apply_transform_ops_to_file,
    };
    use crate::tools::rules_batch::{RulesOp, apply_rules_ops_to_file};
    use crate::tools::sheet_layout::{SheetLayoutOp, apply_sheet_layout_ops_to_file};

    let kind_str = &event.kind.0;
    let payload = &event.payload;

    match kind_str.as_str() {
        // -- write_matrix / edit.batch (existing) --
        "transform.write_matrix" | "edit.batch" => {
            let sheet_name = payload
                .get("sheet_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let anchor = payload
                .get("anchor")
                .and_then(|v| v.as_str())
                .unwrap_or("A1")
                .to_string();
            let overwrite_formulas = payload
                .get("overwrite_formulas")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if let Some(rows_val) = payload.get("rows") {
                let rows = rows_val
                    .as_array()
                    .ok_or_else(|| anyhow!("legacy write_matrix rows must be an array"))?
                    .iter()
                    .map(|row| {
                        row.as_array()
                            .ok_or_else(|| anyhow!("legacy write_matrix row must be an array"))?
                            .iter()
                            .map(|cell| {
                                if cell.is_null() {
                                    Ok(None)
                                } else {
                                    Ok(Some(crate::core::session::SessionMatrixCell::Value(
                                        cell.clone(),
                                    )))
                                }
                            })
                            .collect::<Result<Vec<_>>>()
                    })
                    .collect::<Result<Vec<_>>>()?;

                let ops = vec![SessionTransformOp::WriteMatrix {
                    sheet_name,
                    anchor,
                    rows,
                    overwrite_formulas,
                }];
                session.apply_ops(&ops)?;
            }
        }

        // -- Structure family (insert_rows, delete_rows, clone_row, etc.) --
        k if k.starts_with("structure.") => {
            let ops: Vec<StructureOp> = deserialize_ops_array(payload)?;
            let policy = FormulaParsePolicy::default();
            replay_via_temp_file(session, |path| {
                apply_structure_ops_to_file(path, &ops, policy)?;
                Ok(())
            })?;
        }

        // -- Transform family (clear_range, fill_range, replace_in_range) --
        "transform.clear_range" | "transform.fill_range" | "transform.replace_in_range" => {
            let ops: Vec<TransformOp> = deserialize_ops_array(payload)?;
            replay_via_temp_file(session, |path| {
                apply_transform_ops_to_file(path, &ops)?;
                Ok(())
            })?;
        }

        // -- Style family --
        "style.apply" => {
            let ops: Vec<StyleOp> = deserialize_ops_array(payload)?;
            replay_via_temp_file(session, |path| {
                apply_style_ops_to_file(path, &ops)?;
                Ok(())
            })?;
        }

        // -- Formula pattern family --
        "formula.apply_pattern" => {
            let ops: Vec<ApplyFormulaPatternOpInput> = deserialize_ops_array(payload)?;
            replay_via_temp_file(session, |path| {
                apply_formula_pattern_ops_to_file(path, &ops)?;
                Ok(())
            })?;
        }

        // -- Replace in formulas --
        "formula.replace_in_formulas" => {
            let op: ReplaceInFormulasOp = serde_json::from_value(payload.clone())
                .context("failed to deserialize replace_in_formulas payload")?;
            let policy = FormulaParsePolicy::default();
            replay_via_temp_file(session, |path| {
                apply_replace_in_formulas_to_file(path, &op, policy)?;
                Ok(())
            })?;
        }

        // -- Column sizing family --
        "column.size" => {
            let sheet_name = payload
                .get("sheet_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let ops: Vec<ColumnSizeOp> = deserialize_ops_array(payload)?;
            replay_via_temp_file(session, |path| {
                apply_column_size_ops_to_file(path, &sheet_name, &ops)?;
                Ok(())
            })?;
        }

        // -- Sheet layout family --
        "layout.apply" => {
            let ops: Vec<SheetLayoutOp> = deserialize_ops_array(payload)?;
            replay_via_temp_file(session, |path| {
                apply_sheet_layout_ops_to_file(path, &ops)?;
                Ok(())
            })?;
        }

        // -- Rules family (data validation, conditional formatting) --
        "rules.apply" => {
            let ops: Vec<RulesOp> = deserialize_ops_array(payload)?;
            let policy = FormulaParsePolicy::default();
            replay_via_temp_file(session, |path| {
                apply_rules_ops_to_file(path, &ops, policy)?;
                Ok(())
            })?;
        }

        // -- Name family (direct session mutations, no temp file) --
        "name.define" => {
            let name = payload.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let refers_to = payload
                .get("refers_to")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let scope = payload.get("scope").and_then(|v| v.as_str());
            let scope_sheet = payload.get("scope_sheet_name").and_then(|v| v.as_str());
            session.define_name(name, refers_to, scope, scope_sheet)?;
        }
        "name.update" => {
            let name = payload.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let refers_to = payload.get("refers_to").and_then(|v| v.as_str());
            let scope = payload.get("scope").and_then(|v| v.as_str());
            let scope_sheet = payload.get("scope_sheet_name").and_then(|v| v.as_str());
            session.update_name(name, refers_to, scope, scope_sheet)?;
        }
        "name.delete" => {
            let name = payload.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let scope = payload.get("scope").and_then(|v| v.as_str());
            let scope_sheet = payload.get("scope_sheet_name").and_then(|v| v.as_str());
            session.delete_name(name, scope, scope_sheet)?;
        }

        // -- Session meta events --
        "session.materialize" => {
            // No-op — meta-event recorded for audit purposes.
        }

        _ => {
            tracing::warn!(
                "replay: unsupported event kind '{}' (op_id: {}), skipping",
                kind_str,
                event.op_id
            );
        }
    }

    Ok(())
}

#[cfg(not(feature = "recalc"))]
fn replay_event_on_session(
    _session: &mut crate::core::session::WorkbookSession,
    _event: &OpEvent,
) -> Result<()> {
    bail!("session event replay requires the recalc feature")
}

/// Deserialize an ops array from an event payload.
///
/// Tries `payload["ops"]` first (the standard `{"ops": [...]}` envelope used by
/// batch commands). Falls back to wrapping the entire payload as a single-element
/// vec for events that store a flat operation object.
#[cfg(feature = "recalc")]
fn deserialize_ops_array<T: serde::de::DeserializeOwned>(
    payload: &serde_json::Value,
) -> Result<Vec<T>> {
    if let Some(ops_val) = payload.get("ops") {
        serde_json::from_value(ops_val.clone())
            .context("failed to deserialize ops array from payload")
    } else {
        // Single-op shorthand: wrap the entire payload into a one-element vec.
        let single: T = serde_json::from_value(payload.clone())
            .context("failed to deserialize single op from payload")?;
        Ok(vec![single])
    }
}

// ---------------------------------------------------------------------------
// Precondition evaluation
// ---------------------------------------------------------------------------

/// Evaluate `cell_matches` preconditions against current workbook state.
///
/// Returns a list of violation descriptions (empty = all passed).
fn evaluate_cell_matches(
    session: &crate::core::session::WorkbookSession,
    cell_matches: &[crate::core::events::CellMatch],
) -> Result<Vec<String>> {
    let mut violations = Vec::new();

    for cm in cell_matches {
        let (sheet_name, cell_ref) = if let Some(pos) = cm.address.rfind('!') {
            (&cm.address[..pos], &cm.address[pos + 1..])
        } else {
            return Err(anyhow!(
                "cell_matches address '{}' missing Sheet!Cell notation",
                cm.address
            ));
        };

        let sheet = match session.sheet_by_name(sheet_name) {
            Some(s) => s,
            None => {
                violations.push(format!("{}: sheet '{}' not found", cm.address, sheet_name));
                continue;
            }
        };

        let actual_value = sheet
            .get_cell(cell_ref)
            .map(|c| {
                let val = c.get_value();
                if val.is_empty() {
                    serde_json::Value::Null
                } else if let Ok(n) = val.parse::<f64>() {
                    serde_json::json!(n)
                } else if val == "TRUE" || val == "true" {
                    serde_json::Value::Bool(true)
                } else if val == "FALSE" || val == "false" {
                    serde_json::Value::Bool(false)
                } else {
                    serde_json::Value::String(val.to_string())
                }
            })
            .unwrap_or(serde_json::Value::Null);

        let expected = &cm.value;

        // Compare with tolerance for numbers
        let matches = match (expected, &actual_value) {
            (serde_json::Value::Number(e), serde_json::Value::Number(a)) => {
                let ef = e.as_f64().unwrap_or(f64::NAN);
                let af = a.as_f64().unwrap_or(f64::NAN);
                (ef - af).abs() < 1e-9
            }
            (serde_json::Value::Null, serde_json::Value::Null) => true,
            _ => expected == &actual_value,
        };

        if !matches {
            violations.push(format!(
                "{}: expected {}, got {}",
                cm.address, expected, actual_value
            ));
        }
    }

    Ok(violations)
}

/// Compute a SHA-256 hash of raw workbook bytes.
fn compute_workbook_hash(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_session_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let rand_suffix: u32 = rand::random();
    format!("sess_{:010x}_{:06x}", ts, rand_suffix & 0xFFFFFF)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::{Actor, OpEvent, OpKind};
    use serde_json::json;
    use tempfile::TempDir;

    fn test_actor() -> Actor {
        Actor {
            id: "test:agent".to_string(),
            run_id: None,
            source: "test".to_string(),
        }
    }

    fn create_test_base(dir: &Path) -> PathBuf {
        let base_path = dir.join("base.xlsx");
        let workbook = umya_spreadsheet::new_file();
        umya_spreadsheet::writer::xlsx::write(&workbook, &base_path).unwrap();
        base_path
    }

    #[test]
    fn session_lifecycle_create_and_list() {
        let tmp = TempDir::new().unwrap();
        let base = create_test_base(tmp.path());

        let store = SessionStore::open(tmp.path()).unwrap();
        let handle = store.create_session(&base, Some("Test Session")).unwrap();

        let sessions = store.list_sessions().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0], handle.session_id);

        let meta = handle.meta().unwrap();
        assert_eq!(meta.label.as_deref(), Some("Test Session"));
    }

    #[test]
    fn session_append_and_read_events() {
        let tmp = TempDir::new().unwrap();
        let base = create_test_base(tmp.path());

        let store = SessionStore::open(tmp.path()).unwrap();
        let handle = store.create_session(&base, None).unwrap();

        // Append first event
        let event1 = OpEvent::new(
            handle.session_id.clone(),
            None, // parent is None (first event, HEAD is empty)
            test_actor(),
            OpKind::edit_batch(),
            json!({"cell": "A1", "value": 42}),
        );
        let op1_id = event1.op_id.clone();
        handle.append_event(event1).unwrap();

        assert_eq!(
            handle.read_head().unwrap().as_deref(),
            Some(op1_id.as_str())
        );

        // Append second event
        let event2 = OpEvent::new(
            handle.session_id.clone(),
            Some(op1_id.clone()),
            test_actor(),
            OpKind::edit_batch(),
            json!({"cell": "B2", "value": 100}),
        );
        let op2_id = event2.op_id.clone();
        handle.append_event(event2).unwrap();

        assert_eq!(
            handle.read_head().unwrap().as_deref(),
            Some(op2_id.as_str())
        );

        let events = handle.read_events().unwrap();
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn session_undo_redo() {
        let tmp = TempDir::new().unwrap();
        let base = create_test_base(tmp.path());

        let store = SessionStore::open(tmp.path()).unwrap();
        let handle = store.create_session(&base, None).unwrap();

        let event1 = OpEvent::new(
            handle.session_id.clone(),
            None,
            test_actor(),
            OpKind::edit_batch(),
            json!({"cell": "A1"}),
        );
        let op1_id = event1.op_id.clone();
        handle.append_event(event1).unwrap();

        let event2 = OpEvent::new(
            handle.session_id.clone(),
            Some(op1_id.clone()),
            test_actor(),
            OpKind::edit_batch(),
            json!({"cell": "B2"}),
        );
        let op2_id = event2.op_id.clone();
        handle.append_event(event2).unwrap();

        // Undo back to event1
        let undone = handle.undo().unwrap();
        assert_eq!(undone.as_deref(), Some(op1_id.as_str()));
        assert_eq!(
            handle.read_head().unwrap().as_deref(),
            Some(op1_id.as_str())
        );

        // Undo back to base
        let undone = handle.undo().unwrap();
        assert!(undone.is_none());
        assert!(handle.read_head().unwrap().is_none());

        // Redo to event1
        let redone = handle.redo().unwrap();
        assert_eq!(redone.as_deref(), Some(op1_id.as_str()));

        // Redo to event2
        let redone = handle.redo().unwrap();
        assert_eq!(redone.as_deref(), Some(op2_id.as_str()));
    }

    #[test]
    fn session_cas_conflict() {
        let tmp = TempDir::new().unwrap();
        let base = create_test_base(tmp.path());

        let store = SessionStore::open(tmp.path()).unwrap();
        let handle = store.create_session(&base, None).unwrap();

        let event1 = OpEvent::new(
            handle.session_id.clone(),
            None,
            test_actor(),
            OpKind::edit_batch(),
            json!({}),
        );
        let _op1_id = event1.op_id.clone();
        handle.append_event(event1).unwrap();

        // Try to append with wrong parent (CAS conflict)
        let bad_event = OpEvent::new(
            handle.session_id.clone(),
            None, // Wrong: should be Some(op1_id)
            test_actor(),
            OpKind::edit_batch(),
            json!({}),
        );
        let result = handle.append_event(bad_event);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("CAS conflict"));
    }

    #[test]
    fn session_branching() {
        let tmp = TempDir::new().unwrap();
        let base = create_test_base(tmp.path());

        let store = SessionStore::open(tmp.path()).unwrap();
        let handle = store.create_session(&base, None).unwrap();

        let event1 = OpEvent::new(
            handle.session_id.clone(),
            None,
            test_actor(),
            OpKind::edit_batch(),
            json!({}),
        );
        let op1_id = event1.op_id.clone();
        handle.append_event(event1).unwrap();

        // Create a branch
        handle
            .create_branch("alt-scenario", Some(&op1_id), Some("Alternative"))
            .unwrap();

        let branches = handle.list_branches().unwrap();
        assert_eq!(branches.len(), 2);

        // Switch to alt branch
        handle.switch_branch("alt-scenario").unwrap();
        assert_eq!(handle.current_branch().unwrap(), "alt-scenario");
    }

    #[test]
    fn session_materialize_at_base() {
        let tmp = TempDir::new().unwrap();
        let base = create_test_base(tmp.path());

        let store = SessionStore::open(tmp.path()).unwrap();
        let handle = store.create_session(&base, None).unwrap();

        // Materialize with no events = base file
        let bytes = handle.materialize().unwrap();
        assert!(!bytes.is_empty());
    }

    #[test]
    fn session_delete() {
        let tmp = TempDir::new().unwrap();
        let base = create_test_base(tmp.path());

        let store = SessionStore::open(tmp.path()).unwrap();
        let handle = store.create_session(&base, None).unwrap();
        let sid = handle.session_id.clone();

        store.delete_session(&sid).unwrap();
        assert!(store.list_sessions().unwrap().is_empty());
    }
}
