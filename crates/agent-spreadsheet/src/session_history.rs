//! Canonical resident ownership/history projection, shared by all bindings.
use crate::{
    core::resident_storage::ResidentTransition,
    operations::{CanonicalResponse, ResourceId},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionHistoryRequest {
    Status {
        resource_id: ResourceId,
    },
    Outcome {
        resource_id: ResourceId,
        request_id: String,
    },
    List {
        resource_id: ResourceId,
        #[serde(default)]
        offset: u32,
        #[serde(default = "default_limit")]
        limit: u32,
    },
    Undo {
        resource_id: ResourceId,
        expected_revision: String,
    },
    Redo {
        resource_id: ResourceId,
        expected_revision: String,
    },
    Checkout {
        resource_id: ResourceId,
        expected_revision: String,
        target_commit_id: String,
    },
    CreateBranch {
        resource_id: ResourceId,
        expected_revision: String,
        name: String,
        // Mutation commit ID or "base"; omission selects current head.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(length(min = 1, max = 256))]
        target_commit_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(length(max = 1024))]
        label: Option<String>,
    },
    SwitchBranch {
        resource_id: ResourceId,
        expected_revision: String,
        name: String,
    },
}
fn default_limit() -> u32 {
    100
}
impl SessionHistoryRequest {
    pub fn resource_id(&self) -> &ResourceId {
        match self {
            Self::Status { resource_id }
            | Self::Outcome { resource_id, .. }
            | Self::List { resource_id, .. }
            | Self::Undo { resource_id, .. }
            | Self::Redo { resource_id, .. }
            | Self::Checkout { resource_id, .. }
            | Self::CreateBranch { resource_id, .. }
            | Self::SwitchBranch { resource_id, .. } => resource_id,
        }
    }
    pub fn is_diagnostic(&self) -> bool {
        matches!(self, Self::Status { .. } | Self::Outcome { .. })
    }
    pub fn risk(&self) -> crate::operations::OperationRisk {
        use crate::operations::OperationRisk;
        match self {
            Self::Status { .. } | Self::Outcome { .. } | Self::List { .. } => OperationRisk::Low,
            Self::CreateBranch { .. } => OperationRisk::Moderate,
            _ => OperationRisk::Destructive,
        }
    }
}
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionHealth {
    Usable,
    Poisoned,
}
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RequestOutcomeState {
    Committed,
    NotFound,
    Unknown,
}
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionHistoryEntry {
    pub sequence: u64,
    pub commit_id: String,
    pub request_id: String,
    pub transition: ResidentTransition,
    pub op_kinds: Vec<String>,
    pub history_parent_commit_id: Option<String>,
    pub resulting_head: Option<String>,
    pub branch: String,
    pub revision_id: String,
}
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionHistoryData {
    Status {
        health: SessionHealth,
        revision_id: Option<String>,
        poisoned_request_id: Option<String>,
        reason: Option<String>,
    },
    Outcome {
        request_id: String,
        state: RequestOutcomeState,
        response: Option<CanonicalResponse>,
    },
    List {
        revision_id: String,
        head: Option<String>,
        branch: String,
        branches: BTreeMap<String, Option<String>>,
        branch_labels: BTreeMap<String, String>,
        records: Vec<SessionHistoryEntry>,
        total: usize,
        next_offset: Option<u32>,
    },
    Mutation {
        transition: ResidentTransition,
        revision_before: String,
        revision_after: String,
        head: Option<String>,
        branch: String,
    },
}
