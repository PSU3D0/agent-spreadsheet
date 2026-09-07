//! Host-owned persistence boundary for resident commit streams.
//! Spreadsheet semantics prepare deterministic records; storage owns CAS and
//! durability. Engine state and calculation caches are never journal authority.

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const RESIDENT_COMMIT_SCHEMA: &str = "resident.commit.v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResidentTransition {
    Mutation,
    StageApply,
    CatalogStage,
    CatalogDiscard,
    CalculationPublish,
    CalculationInvalidate,
    Undo,
    Redo,
    Checkout,
    BranchCreate,
    BranchSwitch,
    Checkpoint,
    CheckpointDelete,
    Receipt,
    Restart,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedResidentCommit {
    pub schema_version: String,
    pub session_id: String,
    pub commit_id: String,
    /// Previous record in the append-only stream (never document ancestry).
    pub parent_commit_id: Option<String>,
    /// Parent document mutation for mutation/stage-apply records.
    pub history_parent_commit_id: Option<String>,
    pub base_sha256: String,
    pub request_id: String,
    pub request_fingerprint: String,
    pub transition: ResidentTransition,
    /// Exact successful prepared effects. Non-atomic records contain no failed
    /// or skipped effects.
    pub effects: Vec<Value>,
    pub resulting_head: Option<String>,
    pub resulting_branch: String,
    pub resulting_branches: BTreeMap<String, Option<String>>,
    pub catalog_generation: u64,
    pub state_revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DurableCommitOutcome {
    pub commit_id: String,
    pub sequence: u64,
}

#[derive(Debug, Clone)]
pub struct PortableHistoryState {
    pub session_id: String,
    pub base_sha256: String,
    pub stream_tip: Option<String>,
    pub head: Option<String>,
    pub current_branch: String,
    pub branches: BTreeMap<String, Option<String>>,
    pub catalog_generation: u64,
    pub state_revision: Option<String>,
    pub checkpoints: BTreeMap<String, ResidentCheckpoint>,
    history_parents: BTreeMap<String, Option<String>>,
    catalog_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResidentCheckpoint {
    pub id: String,
    pub head: Option<String>,
    pub state_revision: String,
    pub label: Option<String>,
}

impl PortableHistoryState {
    pub fn empty(session_id: impl Into<String>, base_sha256: impl Into<String>) -> Self {
        let mut branches = BTreeMap::new();
        branches.insert("main".into(), None);
        Self {
            session_id: session_id.into(),
            base_sha256: base_sha256.into(),
            stream_tip: None,
            head: None,
            current_branch: "main".into(),
            branches,
            catalog_generation: 0,
            state_revision: None,
            checkpoints: BTreeMap::new(),
            history_parents: BTreeMap::new(),
            catalog_ids: BTreeSet::new(),
        }
    }

    pub fn replay_bound(
        records: &[PreparedResidentCommit],
        session_id: &str,
        base_sha256: &str,
    ) -> Result<Self> {
        if records
            .iter()
            .any(|record| record.session_id != session_id || record.base_sha256 != base_sha256)
        {
            anyhow::bail!("resident history expected session/base identity mismatch");
        }
        if records.is_empty() {
            return Ok(Self::empty(session_id, base_sha256));
        }
        Self::replay_validated_identity(records)
    }

    /// Inspect a self-identifying stream. Owners must use replay_bound with
    /// their immutable resource/base configuration instead.
    pub fn replay(records: &[PreparedResidentCommit]) -> Result<Self> {
        let first = records
            .first()
            .ok_or_else(|| anyhow::anyhow!("history stream is empty"))?;
        Self::replay_bound(records, &first.session_id, &first.base_sha256)
    }

    fn replay_validated_identity(records: &[PreparedResidentCommit]) -> Result<Self> {
        let first = records
            .first()
            .ok_or_else(|| anyhow::anyhow!("history stream is empty"))?;
        let mut state = Self::empty(&first.session_id, &first.base_sha256);
        let mut commit_ids = BTreeSet::new();
        let mut request_ids = BTreeSet::new();
        let mut visible_states = BTreeSet::new();
        for record in records {
            if record.schema_version != RESIDENT_COMMIT_SCHEMA {
                anyhow::bail!(
                    "unsupported required resident commit schema '{}'",
                    record.schema_version
                );
            }
            if record.session_id != state.session_id || record.base_sha256 != state.base_sha256 {
                anyhow::bail!("resident history session/base identity mismatch");
            }
            if record.commit_id.is_empty() || !commit_ids.insert(record.commit_id.clone()) {
                anyhow::bail!("duplicate or empty resident commit id");
            }
            if record.request_id.is_empty() || !request_ids.insert(record.request_id.clone()) {
                anyhow::bail!("duplicate or empty resident request id in stream");
            }
            if record.parent_commit_id != state.stream_tip {
                anyhow::bail!("resident linear predecessor mismatch");
            }
            crate::canonical_write::validate_portable_prepared_record(
                record,
                state.state_revision.as_deref(),
            )?;
            let publishes_document = record
                .effects
                .iter()
                .find_map(|e| e.pointer("/prepared_transaction/publication/kind"))
                .and_then(Value::as_str)
                .is_some_and(|kind| kind != "none");
            let head_before = state.head.clone();
            if record.history_parent_commit_id.is_some()
                && !matches!(
                    record.transition,
                    ResidentTransition::Mutation | ResidentTransition::StageApply
                )
            {
                anyhow::bail!("control transition cannot have document ancestry");
            }
            if matches!(
                record.transition,
                ResidentTransition::Undo | ResidentTransition::Redo | ResidentTransition::Checkout
            ) {
                let target = record
                    .effects
                    .iter()
                    .find_map(|effect| effect.get("target_head"))
                    .ok_or_else(|| anyhow::anyhow!("history control lacks prepared target"))?;
                if serde_json::from_value::<Option<String>>(target.clone())?
                    != record.resulting_head
                {
                    anyhow::bail!("history control target/result mismatch");
                }
            }
            if record.transition == ResidentTransition::Restart
                && !record
                    .effects
                    .iter()
                    .any(|e| e.get("restart_boundary").and_then(Value::as_bool) == Some(true))
            {
                anyhow::bail!("restart lacks explicit boundary");
            }
            match record.transition {
                ResidentTransition::Receipt => {
                    if let Some(operation) = record
                        .effects
                        .iter()
                        .find_map(|effect| effect.get("receipt_operation"))
                    {
                        match serde_json::from_value::<ResidentTransition>(operation.clone())? {
                            ResidentTransition::Undo if state.head.is_none() => {}
                            ResidentTransition::Redo if state.redo_target()?.is_none() => {}
                            ResidentTransition::CatalogDiscard => {
                                let id = record
                                    .effects
                                    .iter()
                                    .find_map(|e| e.get("discard_staged_change_id"))
                                    .and_then(Value::as_str)
                                    .ok_or_else(|| {
                                        anyhow::anyhow!("discard receipt lacks identity")
                                    })?;
                                if state.catalog_ids.contains(id) {
                                    anyhow::bail!("discard receipt would have catalog effects");
                                }
                            }
                            ResidentTransition::CalculationInvalidate => {}
                            _ => anyhow::bail!("invalid no-op receipt operation/state"),
                        }
                    } else if record
                        .effects
                        .iter()
                        .all(|effect| effect.get("prepared_transaction").is_none())
                    {
                        anyhow::bail!("receipt lacks persisted outcome");
                    }
                }
                ResidentTransition::Mutation | ResidentTransition::StageApply => {
                    if !record
                        .effects
                        .iter()
                        .any(|effect| effect.get("prepared_transaction").is_some())
                    {
                        anyhow::bail!("mutation lacks prepared transaction");
                    }
                    if publishes_document {
                        if record.history_parent_commit_id != state.head {
                            anyhow::bail!("resident document-history parent mismatch");
                        }
                        state.history_parents.insert(
                            record.commit_id.clone(),
                            record.history_parent_commit_id.clone(),
                        );
                        state.head = Some(record.commit_id.clone());
                        state
                            .branches
                            .insert(state.current_branch.clone(), state.head.clone());
                    } else if record.history_parent_commit_id.is_some() {
                        anyhow::bail!("catalog-only staged apply cannot have document ancestry");
                    }
                    if record.transition == ResidentTransition::StageApply {
                        let change_id = record
                            .effects
                            .iter()
                            .find_map(|effect| {
                                effect.pointer("/prepared_transaction/consume_staged")
                            })
                            .and_then(Value::as_str)
                            .ok_or_else(|| {
                                anyhow::anyhow!("stage-apply lacks consumed staged identity")
                            })?;
                        if !state.catalog_ids.remove(change_id) {
                            anyhow::bail!("stage-apply references missing staged change");
                        }
                        state.catalog_generation += 1;
                    }
                }
                ResidentTransition::CatalogStage => {
                    if record.history_parent_commit_id.is_some() {
                        anyhow::bail!("catalog transition cannot have a history parent");
                    }
                    let change_id = record
                        .effects
                        .iter()
                        .find_map(|effect| effect.pointer("/prepared_transaction/staged/0"))
                        .and_then(Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("catalog-stage lacks staged identity"))?;
                    if !state.catalog_ids.insert(change_id.to_string()) {
                        anyhow::bail!("duplicate staged change identity");
                    }
                    state.catalog_generation += 1;
                }
                ResidentTransition::CatalogDiscard => {
                    if record.history_parent_commit_id.is_some() {
                        anyhow::bail!("catalog transition cannot have a history parent");
                    }
                    let change_id = record
                        .effects
                        .iter()
                        .find_map(|effect| effect.get("discard_staged_change_id"))
                        .and_then(Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("catalog-discard lacks staged identity"))?;
                    if !state.catalog_ids.remove(change_id) {
                        anyhow::bail!("catalog-discard references missing staged change");
                    }
                    state.catalog_generation += 1;
                }
                ResidentTransition::CalculationPublish => {
                    if record.history_parent_commit_id.is_some() {
                        anyhow::bail!("non-mutation transition cannot have a history parent");
                    }
                    let snapshot = record
                        .effects
                        .iter()
                        .find_map(|effect| effect.get("calculation_proof"))
                        .ok_or_else(|| anyhow::anyhow!("calculation publication lacks snapshot"))?;
                    let coverage: crate::model::EvaluationCoverage =
                        serde_json::from_value(snapshot.get("coverage").cloned().ok_or_else(
                            || anyhow::anyhow!("calculation publication lacks coverage"),
                        )?)?;
                    if coverage.revision_id != record.state_revision
                        || coverage.evaluated_formula_cells != coverage.formula_cells
                        || coverage.unsupported_formula_cells != 0
                    {
                        anyhow::bail!(
                            "calculation proof is not complete for the resulting revision"
                        );
                    }
                }
                ResidentTransition::CalculationInvalidate => {
                    if record.history_parent_commit_id.is_some()
                        || !record
                            .effects
                            .iter()
                            .any(|effect| effect.get("reason").and_then(Value::as_str).is_some())
                    {
                        anyhow::bail!("invalid calculation-proof revocation");
                    }
                }
                ResidentTransition::Checkpoint => {
                    let label = record
                        .effects
                        .iter()
                        .find_map(|e| e.get("label"))
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    state.checkpoints.insert(
                        record.request_id.clone(),
                        ResidentCheckpoint {
                            id: record.request_id.clone(),
                            head: state.head.clone(),
                            state_revision: record.state_revision.clone(),
                            label,
                        },
                    );
                    state.catalog_generation += 1;
                }
                ResidentTransition::CheckpointDelete => {
                    let id = record
                        .effects
                        .iter()
                        .find_map(|e| e.get("checkpoint_id"))
                        .and_then(Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("checkpoint delete lacks identity"))?;
                    if state.checkpoints.remove(id).is_none() {
                        anyhow::bail!("unknown checkpoint");
                    }
                    state.catalog_generation += 1;
                }
                ResidentTransition::Restart => {
                    if record.history_parent_commit_id.is_some() {
                        anyhow::bail!("non-mutation transition cannot have a history parent");
                    }
                }
                ResidentTransition::Undo => {
                    let expected = state
                        .head
                        .as_ref()
                        .and_then(|head| state.history_parents.get(head))
                        .cloned()
                        .flatten();
                    if record.resulting_head != expected {
                        anyhow::bail!("invalid undo target");
                    }
                    state.head = expected;
                }
                ResidentTransition::Redo | ResidentTransition::Checkout => {
                    let target = record.resulting_head.clone();
                    if let Some(target) = &target {
                        if !state.history_parents.contains_key(target) {
                            anyhow::bail!("history move references unknown mutation");
                        }
                    }
                    if record.transition == ResidentTransition::Redo {
                        let parent = target
                            .as_ref()
                            .and_then(|target| state.history_parents.get(target))
                            .cloned()
                            .flatten();
                        if parent != state.head || target != state.redo_target()? {
                            anyhow::bail!("redo target is not the selected branch successor");
                        }
                    }
                    state.head = target;
                }
                ResidentTransition::BranchCreate => {
                    let name = record
                        .effects
                        .iter()
                        .find_map(|effect| effect.get("branch_name"))
                        .and_then(Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("branch-create lacks branch_name"))?;
                    if state.branches.contains_key(name) {
                        anyhow::bail!("branch already exists");
                    }
                    state.branches.insert(name.to_string(), state.head.clone());
                }
                ResidentTransition::BranchSwitch => {
                    let name = record
                        .effects
                        .iter()
                        .find_map(|effect| effect.get("branch_name"))
                        .and_then(Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("branch-switch lacks branch_name"))?;
                    let target = state.branches.get(name).cloned().ok_or_else(|| {
                        anyhow::anyhow!("branch-switch references unknown branch")
                    })?;
                    state.current_branch = name.to_string();
                    state.head = target;
                }
            }
            if record.resulting_head != state.head
                || record.resulting_branch != state.current_branch
                || record.resulting_branches != state.branches
                || record.catalog_generation != state.catalog_generation
            {
                anyhow::bail!("resident transition resulting state is inconsistent");
            }
            let parse_revision = |revision: &str| -> Result<(String, u64, u64)> {
                let parts = revision.split(':').collect::<Vec<_>>();
                if parts.len() != 4 || parts[0] != "resident" || parts[1].is_empty() {
                    anyhow::bail!("invalid resident revision identity");
                }
                Ok((parts[1].into(), parts[2].parse()?, parts[3].parse()?))
            };
            let next = parse_revision(&record.state_revision)?;
            let previous_revision = state.state_revision.as_deref().or_else(|| {
                record
                    .effects
                    .iter()
                    .find_map(|effect| {
                        effect.pointer("/prepared_transaction/response/revision_before")
                    })
                    .and_then(Value::as_str)
            });
            if let Some(previous) = previous_revision {
                let previous = parse_revision(previous)?;
                let catalog_only = matches!(
                    record.transition,
                    ResidentTransition::CatalogStage
                        | ResidentTransition::CatalogDiscard
                        | ResidentTransition::Checkpoint
                        | ResidentTransition::CheckpointDelete
                        | ResidentTransition::Receipt
                ) || (record.transition == ResidentTransition::StageApply
                    && !publishes_document);
                let document_change = publishes_document
                    || matches!(
                        record.transition,
                        ResidentTransition::Undo
                            | ResidentTransition::Redo
                            | ResidentTransition::Checkout
                    )
                    || (record.transition == ResidentTransition::BranchSwitch
                        && head_before != record.resulting_head);
                if next.0 != previous.0
                    || next.1 != previous.1 + u64::from(document_change)
                    || next.2 != previous.2 + u64::from(!catalog_only)
                {
                    anyhow::bail!("invalid transition-specific resident revision progression");
                }
            }
            if record.transition != ResidentTransition::Receipt
                && !visible_states
                    .insert((record.state_revision.clone(), record.catalog_generation))
            {
                anyhow::bail!("resident observable state ABA detected");
            }
            state.stream_tip = Some(record.commit_id.clone());
            state.state_revision = Some(record.state_revision.clone());
        }
        Ok(state)
    }

    pub fn history_parent(&self, commit_id: &str) -> Result<Option<String>> {
        self.history_parents
            .get(commit_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown document-history commit '{commit_id}'"))
    }

    pub fn redo_target(&self) -> Result<Option<String>> {
        let tip = self.branches.get(&self.current_branch).cloned().flatten();
        let Some(mut cursor) = tip else {
            return Ok(None);
        };
        let mut child = cursor.clone();
        let mut seen = BTreeSet::new();
        loop {
            if !seen.insert(cursor.clone()) {
                anyhow::bail!("resident redo ancestry cycle");
            }
            let parent = self.history_parents.get(&cursor).cloned().ok_or_else(|| {
                anyhow::anyhow!("resident redo ancestry references unknown commit")
            })?;
            if parent == self.head {
                return Ok(Some(child));
            }
            let Some(parent) = parent else {
                return Ok(None);
            };
            child = parent.clone();
            cursor = parent;
        }
    }

    pub fn active_ancestry(&self) -> Result<Vec<String>> {
        let mut result = Vec::new();
        let mut cursor = self.head.clone();
        let mut seen = BTreeSet::new();
        while let Some(id) = cursor {
            if !seen.insert(id.clone()) {
                anyhow::bail!("resident document ancestry cycle");
            }
            result.push(id.clone());
            cursor =
                self.history_parents.get(&id).cloned().ok_or_else(|| {
                    anyhow::anyhow!("resident ancestry references unknown parent")
                })?;
        }
        result.reverse();
        Ok(result)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileOutcome {
    Committed(DurableCommitOutcome),
    NotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeRetention {
    UnlimitedWhileJournalExists,
}

#[async_trait(?Send)]
pub trait ResidentCommitStorage {
    async fn load(&self, session_id: &str) -> Result<Vec<PreparedResidentCommit>>;
    async fn commit(&self, prepared: &PreparedResidentCommit) -> Result<DurableCommitOutcome>;
    async fn reconcile(
        &self,
        session_id: &str,
        request_id: &str,
        request_fingerprint: &str,
    ) -> Result<ReconcileOutcome>;
    fn outcome_retention(&self) -> OutcomeRetention;
}

#[cfg(feature = "native-fs")]
pub mod native {
    use super::*;
    use anyhow::{Context, anyhow, bail};
    use fs2::FileExt;
    use sha2::{Digest, Sha256};
    use std::fs::{self, File, OpenOptions};
    use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct JournalEnvelope {
        sequence: u64,
        record: PreparedResidentCommit,
        previous_hash: Option<String>,
        hash: String,
    }

    pub struct NativeResidentJournal {
        root: PathBuf,
        // Deterministic fault injection for the commit-after-fsync/before-ack branch.
        fail_before_write_once: AtomicBool,
        fail_sync_once: AtomicBool,
        fail_after_commit_once: AtomicBool,
        fail_file_sync: AtomicBool,
        fail_directory_sync: AtomicBool,
    }

    impl NativeResidentJournal {
        pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
            let root = root.into();
            if root.exists() {
                let metadata = fs::symlink_metadata(&root)?;
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    bail!("resident journal root must be a real directory");
                }
            } else {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    fs::DirBuilder::new()
                        .recursive(false)
                        .mode(0o700)
                        .create(&root)?;
                }
                #[cfg(not(unix))]
                fs::create_dir(&root)?;
                #[cfg(unix)]
                if let Some(parent) = root.parent() {
                    File::open(parent)?.sync_all()?;
                }
            }
            Ok(Self {
                root,
                fail_before_write_once: AtomicBool::new(false),
                fail_sync_once: AtomicBool::new(false),
                fail_after_commit_once: AtomicBool::new(false),
                fail_file_sync: AtomicBool::new(false),
                fail_directory_sync: AtomicBool::new(false),
            })
        }

        #[cfg(test)]
        pub(crate) fn fail_before_write_once(&self) {
            self.fail_before_write_once.store(true, Ordering::SeqCst);
        }

        #[cfg(test)]
        pub(crate) fn fail_sync_once(&self) {
            self.fail_sync_once.store(true, Ordering::SeqCst);
        }

        #[cfg(test)]
        pub(crate) fn fail_after_commit_once(&self) {
            self.fail_after_commit_once.store(true, Ordering::SeqCst);
        }

        fn paths(&self, session_id: &str) -> Result<(PathBuf, PathBuf)> {
            if session_id.is_empty()
                || !session_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
            {
                bail!("invalid session id for journal path");
            }
            Ok((
                self.root.join(format!("{session_id}.journal")),
                self.root.join(format!("{session_id}.lock")),
            ))
        }

        fn validate_opened_file(file: &File) -> Result<()> {
            let metadata = file.metadata()?;
            if !metadata.is_file() {
                bail!("opened journal must be a regular file");
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o077 != 0 {
                    bail!("opened journal must be private");
                }
            }
            Ok(())
        }

        fn validate_private_file(path: &std::path::Path) -> Result<()> {
            let metadata = match fs::symlink_metadata(path) {
                Ok(metadata) => Some(metadata),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            if let Some(metadata) = metadata {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    bail!("resident journal path must be a regular file");
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if metadata.permissions().mode() & 0o077 != 0 {
                        bail!("resident journal files must be private");
                    }
                }
            }
            Ok(())
        }

        fn lock(&self, session_id: &str) -> Result<File> {
            let (_, path) = self.paths(session_id)?;
            Self::validate_private_file(&path)?;
            let mut options = OpenOptions::new();
            options.create(true).read(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
                options.custom_flags(libc::O_NOFOLLOW);
            }
            let file = options.open(path)?;
            Self::validate_opened_file(&file)?;
            file.lock_exclusive()?;
            Ok(file)
        }

        fn load_envelopes_with_boundary(
            &self,
            session_id: &str,
        ) -> Result<(Vec<JournalEnvelope>, u64, u64)> {
            let (path, _) = self.paths(session_id)?;
            Self::validate_private_file(&path)?;
            if !path.exists() {
                return Ok((Vec::new(), 0, 0));
            }
            let mut raw = Vec::new();
            let mut options = OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NOFOLLOW);
            }
            let mut file = options.open(&path)?;
            Self::validate_opened_file(&file)?;
            file.read_to_end(&mut raw)?;
            let complete_len = raw
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map_or(0, |i| i + 1);
            // An incomplete final tail is recoverable. Any malformed complete line
            // (including interior corruption) is fatal.
            let complete = &raw[..complete_len];
            let mut envelopes = Vec::new();
            let mut previous_hash: Option<String> = None;
            for (index, line) in BufReader::new(complete).lines().enumerate() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                let envelope: JournalEnvelope = serde_json::from_str(&line)
                    .with_context(|| format!("corrupt journal record at line {}", index + 1))?;
                if envelope.record.schema_version != RESIDENT_COMMIT_SCHEMA {
                    bail!(
                        "unsupported required resident commit schema '{}'",
                        envelope.record.schema_version
                    );
                }
                if envelope.record.session_id != session_id {
                    bail!("journal record belongs to a different session");
                }
                if envelope.sequence != envelopes.len() as u64 + 1
                    || envelope.previous_hash != previous_hash
                    || envelope.hash
                        != envelope_hash(
                            envelope.sequence,
                            &envelope.record,
                            envelope.previous_hash.as_deref(),
                        )?
                {
                    bail!(
                        "resident journal integrity failure at sequence {}",
                        envelope.sequence
                    );
                }
                previous_hash = Some(envelope.hash.clone());
                envelopes.push(envelope);
            }
            if !envelopes.is_empty() {
                PortableHistoryState::replay(
                    &envelopes
                        .iter()
                        .map(|entry| entry.record.clone())
                        .collect::<Vec<_>>(),
                )?;
            }
            Ok((envelopes, complete_len as u64, raw.len() as u64))
        }

        fn load_envelopes(&self, session_id: &str) -> Result<Vec<JournalEnvelope>> {
            Ok(self.load_envelopes_with_boundary(session_id)?.0)
        }

        fn sync_file(&self, file: &File) -> Result<()> {
            if self.fail_file_sync.load(Ordering::SeqCst) {
                bail!("persistent file sync failure; outcome uncertain");
            }
            file.sync_all()?;
            Ok(())
        }

        fn sync_directory(&self) -> Result<()> {
            if self.fail_directory_sync.load(Ordering::SeqCst) {
                bail!("persistent directory sync failure; outcome uncertain");
            }
            #[cfg(unix)]
            File::open(&self.root)?.sync_all()?;
            Ok(())
        }
    }

    fn envelope_hash(
        sequence: u64,
        record: &PreparedResidentCommit,
        previous: Option<&str>,
    ) -> Result<String> {
        let bytes = serde_json::to_vec(&(sequence, record, previous))?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }

    #[async_trait(?Send)]
    impl ResidentCommitStorage for NativeResidentJournal {
        async fn load(&self, session_id: &str) -> Result<Vec<PreparedResidentCommit>> {
            let records = self
                .load_envelopes(session_id)?
                .into_iter()
                .map(|entry| entry.record)
                .collect::<Vec<_>>();
            if !records.is_empty() {
                PortableHistoryState::replay(&records)?;
            }
            Ok(records)
        }

        async fn commit(&self, prepared: &PreparedResidentCommit) -> Result<DurableCommitOutcome> {
            if prepared.schema_version != RESIDENT_COMMIT_SCHEMA {
                bail!(
                    "unsupported resident commit schema '{}': prepare refused",
                    prepared.schema_version
                );
            }
            let lock = self.lock(&prepared.session_id)?;
            let (entries, complete_len, physical_len) =
                self.load_envelopes_with_boundary(&prepared.session_id)?;
            if entries
                .iter()
                .any(|entry| entry.record.base_sha256 != prepared.base_sha256)
            {
                bail!("journal expected base identity mismatch");
            }
            if let Some(existing) = entries
                .iter()
                .find(|entry| entry.record.request_id == prepared.request_id)
            {
                if existing.record.request_fingerprint != prepared.request_fingerprint {
                    bail!("request identity reuse with a different fingerprint");
                }
                let outcome = DurableCommitOutcome {
                    commit_id: existing.record.commit_id.clone(),
                    sequence: existing.sequence,
                };
                let (path, _) = self.paths(&prepared.session_id)?;
                Self::validate_private_file(&path)?;
                let mut options = OpenOptions::new();
                options.read(true).write(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.custom_flags(libc::O_NOFOLLOW);
                }
                let file = options.open(path)?;
                Self::validate_opened_file(&file)?;
                if physical_len != complete_len {
                    file.set_len(complete_len)?;
                }
                self.sync_file(&file)?;
                self.sync_directory()?;
                lock.unlock()?;
                return Ok(outcome);
            }
            let parent = entries.last().map(|entry| entry.record.commit_id.as_str());
            if prepared.parent_commit_id.as_deref() != parent {
                bail!(
                    "journal CAS conflict: expected parent {:?}, current {:?}",
                    prepared.parent_commit_id,
                    parent
                );
            }
            let mut logical_records = entries
                .iter()
                .map(|entry| entry.record.clone())
                .collect::<Vec<_>>();
            logical_records.push(prepared.clone());
            PortableHistoryState::replay_bound(
                &logical_records,
                &prepared.session_id,
                &prepared.base_sha256,
            )?;
            let sequence = entries.len() as u64 + 1;
            let previous_hash = entries.last().map(|entry| entry.hash.clone());
            let envelope = JournalEnvelope {
                sequence,
                hash: envelope_hash(sequence, prepared, previous_hash.as_deref())?,
                previous_hash,
                record: prepared.clone(),
            };
            if self.fail_before_write_once.swap(false, Ordering::SeqCst) {
                lock.unlock()?;
                return Err(anyhow!("injected failure before journal write"));
            }
            let (path, _) = self.paths(&prepared.session_id)?;
            Self::validate_private_file(&path)?;
            let mut options = OpenOptions::new();
            options.create(true).read(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
                options.custom_flags(libc::O_NOFOLLOW);
            }
            let mut file = options.open(&path)?;
            Self::validate_opened_file(&file)?;
            if physical_len != complete_len {
                file.set_len(complete_len)?;
                self.sync_file(&file)?;
            }
            file.seek(SeekFrom::End(0))?;
            writeln!(file, "{}", serde_json::to_string(&envelope)?)?;
            if self.fail_sync_once.swap(false, Ordering::SeqCst) {
                return Err(anyhow!("injected journal fsync failure; outcome uncertain"));
            }
            self.sync_file(&file)?;
            // A previous uncertain creation may already be visible. Re-establish
            // the directory barrier even when this append did not create it.
            self.sync_directory()?;
            let outcome = DurableCommitOutcome {
                commit_id: prepared.commit_id.clone(),
                sequence,
            };
            lock.unlock()?;
            if self.fail_after_commit_once.swap(false, Ordering::SeqCst) {
                return Err(anyhow!(
                    "commit outcome unknown after durable append; reconcile request identity"
                ));
            }
            Ok(outcome)
        }

        async fn reconcile(
            &self,
            session_id: &str,
            request_id: &str,
            request_fingerprint: &str,
        ) -> Result<ReconcileOutcome> {
            let lock = self.lock(session_id)?;
            let (entries, complete_len, physical_len) =
                self.load_envelopes_with_boundary(session_id)?;
            let (path, _) = self.paths(session_id)?;
            if path.exists() {
                Self::validate_private_file(&path)?;
                let mut options = OpenOptions::new();
                options.read(true).write(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.custom_flags(libc::O_NOFOLLOW);
                }
                let file = options.open(path)?;
                Self::validate_opened_file(&file)?;
                if physical_len != complete_len {
                    file.set_len(complete_len)?;
                }
                self.sync_file(&file)?;
                self.sync_directory()?;
            }
            let Some(entry) = entries
                .iter()
                .find(|entry| entry.record.request_id == request_id)
            else {
                lock.unlock()?;
                return Ok(ReconcileOutcome::NotFound);
            };
            if entry.record.request_fingerprint != request_fingerprint {
                bail!("request identity fingerprint mismatch");
            }
            lock.unlock()?;
            Ok(ReconcileOutcome::Committed(DurableCommitOutcome {
                commit_id: entry.record.commit_id.clone(),
                sequence: entry.sequence,
            }))
        }

        fn outcome_retention(&self) -> OutcomeRetention {
            OutcomeRetention::UnlimitedWhileJournalExists
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use tempfile::tempdir;

        fn record(parent: Option<&str>, request: &str) -> PreparedResidentCommit {
            PreparedResidentCommit {
                schema_version: RESIDENT_COMMIT_SCHEMA.into(),
                session_id: "sess_test".into(),
                commit_id: format!("commit_{request}"),
                parent_commit_id: parent.map(str::to_string),
                history_parent_commit_id: parent.map(str::to_string),
                base_sha256: "base_test".into(),
                request_id: request.into(),
                request_fingerprint: format!("fp_{request}"),
                transition: ResidentTransition::Mutation,
                effects: vec![serde_json::json!({"prepared_transaction":{
                    "base_sha256":"base_test",
                    "response": {
                        "status":"applied", "mode":"apply", "atomic":true,
                        "revision_before":format!("resident:epoch:{0}:{0}", u64::from(parent.is_some())),
                        "revision_after":format!("resident:epoch:{0}:{0}", 1 + u64::from(parent.is_some())),
                        "ops_applied":1,
                        "diff":{"change_count":1,"exact":true,"precision":"exact","changes":[],"effects":[]},
                        "impact":{"op_kinds":["set_cells"],"risk":"moderate"}, "results":[]
                    },
                    "publication":{"kind":"cells","effects":[{
                        "sheet_name":"Sheet1","column":1,"row":1,"source_op_indices":[0],
                        "expected_before":null,"after":{"formula":null,"value":{"kind":"number","value":2.0}}
                    }]},
                    "staged":null,"consume_staged":null
                }})],
                resulting_head: Some(format!("commit_{request}")),
                resulting_branch: "main".into(),
                resulting_branches: BTreeMap::from([(
                    "main".into(),
                    Some(format!("commit_{request}")),
                )]),
                catalog_generation: 0,
                state_revision: format!(
                    "resident:epoch:{}:{}",
                    if parent.is_some() { 2 } else { 1 },
                    if parent.is_some() { 2 } else { 1 }
                ),
            }
        }

        #[tokio::test]
        async fn persistent_barrier_failures_never_reconcile_as_committed() {
            for directory_failure in [false, true] {
                let dir = tempdir().unwrap();
                let journal = NativeResidentJournal::open(dir.path()).unwrap();
                if directory_failure {
                    journal.fail_directory_sync.store(true, Ordering::SeqCst);
                } else {
                    journal.fail_file_sync.store(true, Ordering::SeqCst);
                }
                let first = record(None, "one");
                assert!(journal.commit(&first).await.is_err());
                assert!(
                    journal
                        .reconcile("sess_test", "one", "fp_one")
                        .await
                        .is_err()
                );
                assert!(journal.commit(&first).await.is_err());
                assert!(
                    journal
                        .commit(&record(Some("commit_one"), "two"))
                        .await
                        .is_err()
                );
                journal.fail_directory_sync.store(false, Ordering::SeqCst);
                journal.fail_file_sync.store(false, Ordering::SeqCst);
                assert!(matches!(
                    journal
                        .reconcile("sess_test", "one", "fp_one")
                        .await
                        .unwrap(),
                    ReconcileOutcome::Committed(_)
                ));
            }
        }

        #[tokio::test]
        async fn commit_then_error_reconciles_and_rejects_identity_reuse() {
            let dir = tempdir().unwrap();
            let journal = NativeResidentJournal::open(dir.path()).unwrap();
            journal.fail_after_commit_once();
            let first = record(None, "one");
            assert!(journal.commit(&first).await.is_err());
            assert_eq!(
                journal
                    .reconcile("sess_test", "one", "fp_one")
                    .await
                    .unwrap(),
                ReconcileOutcome::Committed(DurableCommitOutcome {
                    commit_id: "commit_one".into(),
                    sequence: 1
                })
            );
            let mut reused = first.clone();
            reused.request_fingerprint = "different".into();
            assert!(journal.commit(&reused).await.is_err());
        }

        #[test]
        fn portable_replay_rejects_wrong_base_duplicate_ids_and_unknown_transition() {
            let first = record(None, "one");
            let mut wrong_base = record(Some("commit_one"), "two");
            wrong_base.base_sha256 = "other".into();
            assert!(PortableHistoryState::replay(&[first.clone(), wrong_base]).is_err());
            let mut duplicate = record(Some("commit_one"), "two");
            duplicate.commit_id = "commit_one".into();
            assert!(PortableHistoryState::replay(&[first, duplicate]).is_err());
            assert!(
                serde_json::from_value::<ResidentTransition>(serde_json::json!("future_required"))
                    .is_err()
            );
        }

        #[tokio::test]
        async fn prewrite_and_fsync_faults_have_honest_reconciliation() {
            let dir = tempdir().unwrap();
            let journal = NativeResidentJournal::open(dir.path()).unwrap();
            let first = record(None, "one");
            journal.fail_before_write_once();
            assert!(journal.commit(&first).await.is_err());
            assert_eq!(
                journal
                    .reconcile("sess_test", "one", "fp_one")
                    .await
                    .unwrap(),
                ReconcileOutcome::NotFound
            );
            journal.fail_sync_once();
            assert!(journal.commit(&first).await.is_err());
            assert_eq!(
                journal
                    .reconcile("sess_test", "one", "fp_one")
                    .await
                    .unwrap(),
                ReconcileOutcome::Committed(DurableCommitOutcome {
                    commit_id: "commit_one".into(),
                    sequence: 1,
                })
            );
        }

        #[cfg(unix)]
        #[tokio::test]
        async fn journal_files_are_private_and_symlink_targets_are_rejected() {
            use std::os::unix::fs::{PermissionsExt, symlink};
            let dir = tempdir().unwrap();
            let journal = NativeResidentJournal::open(dir.path()).unwrap();
            journal.commit(&record(None, "one")).await.unwrap();
            let path = dir.path().join("sess_test.journal");
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );

            let target = dir.path().join("target");
            fs::write(&target, b"").unwrap();
            fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
            let missing = dir.path().join("missing-target");
            symlink(&missing, dir.path().join("sess_dangling.journal")).unwrap();
            assert!(journal.load("sess_dangling").await.is_err());
            assert!(!missing.exists());
            assert!(NativeResidentJournal::open(dir.path().join("absent/child")).is_err());
            symlink(&target, dir.path().join("sess_evil.journal")).unwrap();
            let mut evil = record(None, "evil");
            evil.session_id = "sess_evil".into();
            assert!(
                journal
                    .commit(&evil)
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("regular file")
            );
        }

        #[tokio::test]
        async fn parent_cas_allows_only_one_competing_commit() {
            let dir = tempdir().unwrap();
            let journal = NativeResidentJournal::open(dir.path()).unwrap();
            let first = record(None, "one");
            journal.commit(&first).await.unwrap();
            let winner = record(Some("commit_one"), "two");
            let loser = record(Some("commit_one"), "three");
            journal.commit(&winner).await.unwrap();
            assert!(journal.commit(&loser).await.is_err());
            assert_eq!(journal.load("sess_test").await.unwrap().len(), 2);
        }

        #[tokio::test]
        async fn incomplete_tail_recovers_but_unknown_complete_version_fails() {
            let dir = tempdir().unwrap();
            let journal = NativeResidentJournal::open(dir.path()).unwrap();
            journal.commit(&record(None, "one")).await.unwrap();
            let path = dir.path().join("sess_test.journal");
            OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(b"{partial")
                .unwrap();
            assert_eq!(journal.load("sess_test").await.unwrap().len(), 1);
            journal
                .commit(&record(Some("commit_one"), "two"))
                .await
                .unwrap();
            assert_eq!(journal.load("sess_test").await.unwrap().len(), 2);

            let partial_dir = tempdir().unwrap();
            let partial_journal = NativeResidentJournal::open(partial_dir.path()).unwrap();
            let partial_path = partial_dir.path().join("sess_test.journal");
            fs::write(&partial_path, b"{partial").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&partial_path, fs::Permissions::from_mode(0o600)).unwrap();
            }
            partial_journal.commit(&record(None, "one")).await.unwrap();
            assert_eq!(partial_journal.load("sess_test").await.unwrap().len(), 1);

            fs::write(&path, format!("{}\n", serde_json::json!({
                "sequence": 1,
                "record": {
                    "schema_version":"resident.commit.v999","session_id":"sess_test","commit_id":"x",
                    "parent_commit_id":null,"request_id":"r","request_fingerprint":"f",
                    "transition":"mutation","effects":[],"resulting_head":null,
                    "resulting_branch":"main","catalog_generation":0,"state_revision":"s"
                },
                "previous_hash":null,"hash":"bad"
            }))).unwrap();
            assert!(journal.load("sess_test").await.is_err());
        }
    }
}
