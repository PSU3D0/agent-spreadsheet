//! Universal OpEvent envelope and opcode registry for event-sourced sessions.
//!
//! Every effectful action is expressed as a normalized [`OpEvent`], which records
//! intent, provenance, preconditions, and calculated impact. Events are appended
//! to a JSONL binlog and replayed to reconstruct workbook state.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Current schema version for OpEvent serialization.
pub const SCHEMA_VERSION: &str = "ops.v1";

// ---------------------------------------------------------------------------
// OpEvent envelope
// ---------------------------------------------------------------------------

/// Universal event envelope wrapping every effectful workbook operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpEvent {
    /// Schema version tag for forward-compatible parsing.
    pub schema_version: String,

    /// Unique event identifier (ULID-style sorted string).
    pub op_id: String,

    /// Identifier of the previous event in this branch (forms a linked list).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,

    /// Session this event belongs to.
    pub session_id: String,

    /// Wall-clock timestamp of event creation.
    pub timestamp: DateTime<Utc>,

    /// Who or what created this event.
    pub actor: Actor,

    /// Namespaced operation kind (e.g. `structure.clone_row`).
    pub kind: OpKind,

    /// Operation-specific payload (JSON object).
    pub payload: serde_json::Value,

    /// SHA-256 hash over the canonicalized payload JSON.
    pub canonical_payload_hash: String,

    /// Pre-apply validation constraints.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preconditions: Option<Preconditions>,

    /// Computed impact from dry-run staging (populated during stage, before apply).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_run_impact: Option<DryRunImpact>,

    /// Result metadata after applying the event.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apply_result: Option<ApplyResult>,

    /// Hash of the previous event record (for tamper-detection chains).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev_event_hash: Option<String>,

    /// Hash of this entire event record (computed after serialization).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_hash: Option<String>,

    /// Forward-compatible catch-all for unknown fields.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Actor
// ---------------------------------------------------------------------------

/// Identity of the entity that created an event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Actor {
    /// Actor identifier (e.g. `agent:fp_and_a_bot`, `user:jane`).
    pub id: String,

    /// Optional run/session identifier for the actor's execution context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,

    /// Source surface that produced the event (`cli`, `mcp`, `sdk`, `wasm`).
    #[serde(default = "default_source")]
    pub source: String,
}

fn default_source() -> String {
    "cli".to_string()
}

// ---------------------------------------------------------------------------
// OpKind — namespaced operation discriminator
// ---------------------------------------------------------------------------

/// Namespaced operation kind covering all write families.
///
/// The string representation uses dot-separated namespaces, e.g.
/// `structure.insert_rows`, `transform.write_matrix`, `name.define`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OpKind(pub String);

impl OpKind {
    pub fn new(namespace: &str, action: &str) -> Self {
        Self(format!("{}.{}", namespace, action))
    }

    pub fn namespace(&self) -> &str {
        self.0.split('.').next().unwrap_or(&self.0)
    }

    pub fn action(&self) -> &str {
        self.0.split('.').nth(1).unwrap_or("")
    }

    // -- Structure family --
    pub fn structure_insert_rows() -> Self {
        Self::new("structure", "insert_rows")
    }
    pub fn structure_delete_rows() -> Self {
        Self::new("structure", "delete_rows")
    }
    pub fn structure_insert_cols() -> Self {
        Self::new("structure", "insert_cols")
    }
    pub fn structure_delete_cols() -> Self {
        Self::new("structure", "delete_cols")
    }
    pub fn structure_clone_row() -> Self {
        Self::new("structure", "clone_row")
    }
    pub fn structure_merge_cells() -> Self {
        Self::new("structure", "merge_cells")
    }
    pub fn structure_unmerge_cells() -> Self {
        Self::new("structure", "unmerge_cells")
    }
    pub fn structure_rename_sheet() -> Self {
        Self::new("structure", "rename_sheet")
    }
    pub fn structure_create_sheet() -> Self {
        Self::new("structure", "create_sheet")
    }
    pub fn structure_delete_sheet() -> Self {
        Self::new("structure", "delete_sheet")
    }
    pub fn structure_copy_range() -> Self {
        Self::new("structure", "copy_range")
    }
    pub fn structure_move_range() -> Self {
        Self::new("structure", "move_range")
    }

    // -- Transform family --
    pub fn transform_clear_range() -> Self {
        Self::new("transform", "clear_range")
    }
    pub fn transform_fill_range() -> Self {
        Self::new("transform", "fill_range")
    }
    pub fn transform_replace_in_range() -> Self {
        Self::new("transform", "replace_in_range")
    }
    pub fn transform_write_matrix() -> Self {
        Self::new("transform", "write_matrix")
    }

    // -- Style family --
    pub fn style_apply() -> Self {
        Self::new("style", "apply")
    }

    // -- Formula pattern family --
    pub fn formula_apply_pattern() -> Self {
        Self::new("formula", "apply_pattern")
    }
    pub fn formula_replace_in_formulas() -> Self {
        Self::new("formula", "replace_in_formulas")
    }

    // -- Column sizing family --
    pub fn column_size() -> Self {
        Self::new("column", "size")
    }

    // -- Sheet layout family --
    pub fn layout_apply() -> Self {
        Self::new("layout", "apply")
    }

    // -- Rules family --
    pub fn rules_apply() -> Self {
        Self::new("rules", "apply")
    }

    // -- Name family --
    pub fn name_define() -> Self {
        Self::new("name", "define")
    }
    pub fn name_update() -> Self {
        Self::new("name", "update")
    }
    pub fn name_delete() -> Self {
        Self::new("name", "delete")
    }

    // -- Edit family (shorthand cell edits) --
    pub fn edit_batch() -> Self {
        Self::new("edit", "batch")
    }

    // -- Import family --
    pub fn import_range() -> Self {
        Self::new("import", "range")
    }
    pub fn import_grid() -> Self {
        Self::new("import", "grid")
    }

    // -- Session meta --
    pub fn session_materialize() -> Self {
        Self::new("session", "materialize")
    }
}

impl std::fmt::Display for OpKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ---------------------------------------------------------------------------
// Preconditions
// ---------------------------------------------------------------------------

/// Pre-apply validation constraints attached to an event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preconditions {
    /// Cell value assertions that must hold before applying.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cell_matches: Vec<CellMatch>,

    /// Expected workbook content hash before applying.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workbook_hash_before: Option<String>,

    /// The HEAD op_id that this event was staged against.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_at_stage: Option<String>,
}

/// A single cell value assertion for precondition checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CellMatch {
    /// Cell address in `Sheet!A1` notation.
    pub address: String,
    /// Expected cell display value.
    pub value: serde_json::Value,
}

// ---------------------------------------------------------------------------
// DryRunImpact
// ---------------------------------------------------------------------------

/// Computed impact from staging an operation (dry-run analysis).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DryRunImpact {
    pub cells_changed: u64,
    pub formulas_rewritten: u64,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shifted_spans: Vec<ShiftedSpan>,

    pub ref_errors_generated: u64,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub boundary_warnings: Vec<String>,
}

/// Describes a row/column shift caused by a structural operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShiftedSpan {
    pub op_index: usize,
    pub sheet_name: String,
    /// `"row"` or `"col"`.
    pub axis: String,
    pub at: u32,
    pub count: u32,
    /// `"insert"` or `"delete"`.
    pub direction: String,
    pub description: String,
}

// ---------------------------------------------------------------------------
// ApplyResult
// ---------------------------------------------------------------------------

/// Metadata recorded after applying an event to the workbook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyResult {
    /// `applied`, `rejected`, or `superseded`.
    pub status: ApplyStatus,
    pub duration_ms: u64,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub workbook_hash_after: Option<String>,
}

/// Outcome status of applying an event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyStatus {
    Applied,
    Rejected,
    Superseded,
}

// ---------------------------------------------------------------------------
// Canonical hashing
// ---------------------------------------------------------------------------

/// Compute SHA-256 hash over a canonicalized JSON payload.
///
/// Canonicalization: serialize with sorted keys and no extra whitespace.
pub fn canonical_payload_hash(payload: &serde_json::Value) -> String {
    let canonical = canonical_json(payload);
    let hash = Sha256::digest(canonical.as_bytes());
    format!("sha256:{:x}", hash)
}

/// Produce a canonical JSON string with sorted keys.
fn canonical_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let mut sorted: Vec<(&String, &serde_json::Value)> = map.iter().collect();
            sorted.sort_by_key(|(k, _)| *k);
            let entries: Vec<String> = sorted
                .into_iter()
                .map(|(k, v)| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap(),
                        canonical_json(v)
                    )
                })
                .collect();
            format!("{{{}}}", entries.join(","))
        }
        serde_json::Value::Array(arr) => {
            let entries: Vec<String> = arr.iter().map(canonical_json).collect();
            format!("[{}]", entries.join(","))
        }
        _ => serde_json::to_string(value).unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// OpEvent builder
// ---------------------------------------------------------------------------

impl OpEvent {
    /// Create a new event with the given parameters and compute the payload hash.
    pub fn new(
        session_id: String,
        parent_id: Option<String>,
        actor: Actor,
        kind: OpKind,
        payload: serde_json::Value,
    ) -> Self {
        let hash = canonical_payload_hash(&payload);
        let op_id = make_op_id();
        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            op_id,
            parent_id,
            session_id,
            timestamp: Utc::now(),
            actor,
            kind,
            payload,
            canonical_payload_hash: hash,
            preconditions: None,
            dry_run_impact: None,
            apply_result: None,
            prev_event_hash: None,
            event_hash: None,
            extra: BTreeMap::new(),
        }
    }

    /// Attach preconditions to this event.
    pub fn with_preconditions(mut self, preconditions: Preconditions) -> Self {
        self.preconditions = Some(preconditions);
        self
    }

    /// Attach dry-run impact to this event.
    pub fn with_dry_run_impact(mut self, impact: DryRunImpact) -> Self {
        self.dry_run_impact = Some(impact);
        self
    }

    /// Record the apply result.
    pub fn with_apply_result(mut self, result: ApplyResult) -> Self {
        self.apply_result = Some(result);
        self
    }

    pub fn verify_integrity(&self) -> bool {
        if self.canonical_payload_hash != canonical_payload_hash(&self.payload) {
            return false;
        }
        let Some(expected) = &self.event_hash else {
            return false;
        };
        let mut unsigned = self.clone();
        unsigned.event_hash = None;
        let json = serde_json::to_string(&unsigned).unwrap_or_default();
        let actual = format!("sha256:{:x}", Sha256::digest(json.as_bytes()));
        &actual == expected
    }

    /// Compute and set the event hash over the full serialized record.
    pub fn seal(&mut self) {
        let json = serde_json::to_string(self).unwrap_or_default();
        let hash = Sha256::digest(json.as_bytes());
        self.event_hash = Some(format!("sha256:{:x}", hash));
    }

    /// Validate schema version compatibility.
    pub fn validate_version(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "unsupported schema version '{}' (expected '{}')",
                self.schema_version, SCHEMA_VERSION
            ));
        }
        Ok(())
    }
}

/// Generate a time-sortable unique operation ID.
fn make_op_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let rand_suffix: u32 = rand::random();
    format!("op_{:013x}_{:08x}", ts, rand_suffix)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn op_event_roundtrip() {
        let event = OpEvent::new(
            "sess_test".to_string(),
            None,
            Actor {
                id: "test:agent".to_string(),
                run_id: None,
                source: "cli".to_string(),
            },
            OpKind::structure_clone_row(),
            json!({
                "sheet_name": "Provider",
                "source_row": 85,
                "insert_at": 86
            }),
        );

        let serialized = serde_json::to_string(&event).unwrap();
        let deserialized: OpEvent = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.schema_version, SCHEMA_VERSION);
        assert_eq!(deserialized.kind, OpKind::structure_clone_row());
        assert_eq!(deserialized.session_id, "sess_test");
        assert!(deserialized.canonical_payload_hash.starts_with("sha256:"));
    }

    #[test]
    fn canonical_hash_is_deterministic() {
        let payload_a = json!({"b": 2, "a": 1});
        let payload_b = json!({"a": 1, "b": 2});
        assert_eq!(
            canonical_payload_hash(&payload_a),
            canonical_payload_hash(&payload_b)
        );
    }

    #[test]
    fn op_kind_namespace_and_action() {
        let kind = OpKind::structure_clone_row();
        assert_eq!(kind.namespace(), "structure");
        assert_eq!(kind.action(), "clone_row");
        assert_eq!(kind.to_string(), "structure.clone_row");
    }

    #[test]
    fn unknown_version_rejected() {
        let mut event = OpEvent::new(
            "sess_test".to_string(),
            None,
            Actor {
                id: "test".to_string(),
                run_id: None,
                source: "cli".to_string(),
            },
            OpKind::edit_batch(),
            json!({}),
        );
        event.schema_version = "ops.v999".to_string();
        assert!(event.validate_version().is_err());
    }

    #[test]
    fn unknown_fields_tolerated() {
        let json_str = r#"{
            "schema_version": "ops.v1",
            "op_id": "op_test",
            "session_id": "sess_test",
            "timestamp": "2024-10-24T10:00:00Z",
            "actor": {"id": "test", "source": "cli"},
            "kind": "structure.clone_row",
            "payload": {},
            "canonical_payload_hash": "sha256:abc",
            "future_field": "should be preserved"
        }"#;

        let event: OpEvent = serde_json::from_str(json_str).unwrap();
        assert_eq!(
            event.extra.get("future_field").unwrap(),
            "should be preserved"
        );
    }

    #[test]
    fn seal_produces_event_hash() {
        let mut event = OpEvent::new(
            "sess_test".to_string(),
            None,
            Actor {
                id: "test".to_string(),
                run_id: None,
                source: "cli".to_string(),
            },
            OpKind::edit_batch(),
            json!({"cell": "A1", "value": 42}),
        );
        assert!(event.event_hash.is_none());
        event.seal();
        assert!(event.event_hash.is_some());
        assert!(event.event_hash.as_ref().unwrap().starts_with("sha256:"));
    }
}
