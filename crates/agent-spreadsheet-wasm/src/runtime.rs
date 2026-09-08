//! One serialized, thread-confined resident owner per byte-session handle.
use crate::{storage::MemoryJournal, wasm_config};
use agent_spreadsheet::{
    canonical_write::ResidentWriteSession,
    core::session::*,
    model::*,
    operations::{
        CanonicalErrorEnvelope, CanonicalResponse, ResourceId, SpreadsheetOperation,
        decode_operation,
    },
    session::{ResidentSessionRuntime, execute_terminal_session},
};
use anyhow::{Result, anyhow, bail};
use futures::{FutureExt, lock::Mutex};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::{cell::RefCell, rc::Rc};

type Runtime = ResidentSessionRuntime<MemoryJournal>;
struct Owner {
    resource: ResourceId,
    storage: MemoryJournal,
    gate: Mutex<()>,
    runtime: RefCell<Option<Runtime>>,
}
#[derive(Clone)]
pub(crate) struct StoredSession(Rc<Owner>);

macro_rules! read_method {
    ($name:ident($($arg:ident: $ty:ty),*) -> $result:ty) => {
        pub fn $name(&self, $($arg: $ty),*) -> Result<$result> {
            self.read(|document| document.$name($($arg),*))
        }
    };
}
impl StoredSession {
    pub fn new(resource: ResourceId, bytes: &[u8]) -> Result<Self> {
        let id = resource.to_workbook_id().0;
        let storage = MemoryJournal::new(
            id.clone(),
            agent_spreadsheet::utils::hash_bytes_sha256_hex(bytes),
        );
        let owner = ResidentWriteSession::from_bytes(format!("session:{id}"), bytes)?;
        let runtime =
            ResidentSessionRuntime::new(owner, storage.clone(), wasm_config(), resource.clone())?;
        Ok(Self(Rc::new(Owner {
            resource,
            storage,
            gate: Mutex::new(()),
            runtime: RefCell::new(Some(runtime)),
        })))
    }
    fn read<R>(&self, project: impl FnOnce(&WorkbookSession) -> Result<R>) -> Result<R> {
        let owner = self
            .0
            .runtime
            .try_borrow()
            .map_err(|_| anyhow!("session is busy"))?;
        let owner = owner
            .as_ref()
            .ok_or_else(|| anyhow!("resource has been discarded"))?;
        owner.owner.read_view()?;
        project(owner.owner.diagnostic_workbook().legacy_read_session())
    }
    pub fn metadata(&self) -> Result<Value> {
        let owner = self
            .0
            .runtime
            .try_borrow()
            .map_err(|_| anyhow!("session is busy"))?;
        let owner = owner
            .as_ref()
            .ok_or_else(|| anyhow!("resource has been discarded"))?;
        owner.owner.read_view()?;
        let workbook = owner.owner.diagnostic_workbook();
        Ok(
            json!({"resource_id": self.0.resource, "revision_id": owner.owner.revision(),
            "durability": "memory", "evaluator": workbook.evaluator_counters(),
            "serializations": workbook.serialization_count()}),
        )
    }
    pub fn revision(&self) -> Result<String> {
        let owner = self
            .0
            .runtime
            .try_borrow()
            .map_err(|_| anyhow!("session is busy"))?;
        let owner = owner
            .as_ref()
            .ok_or_else(|| anyhow!("resource has been discarded"))?;
        owner.owner.read_view()?;
        Ok(owner.owner.revision())
    }
    pub fn list_sheets(&self) -> Result<Vec<String>> {
        self.read(|document| Ok(document.list_sheets()))
    }
    read_method!(describe_workbook() -> WorkbookDescription);
    read_method!(named_ranges() -> NamedRangesResponse);
    read_method!(sheet_overview(params: SessionSheetOverviewParams) -> SheetOverviewResponse);
    read_method!(find_value(params: SessionFindValueParams) -> FindValueResponse);
    read_method!(read_table(params: SessionReadTableParams) -> ReadTableResponse);
    read_method!(sheet_page(params: SessionSheetPageParams) -> SheetPageResponse);
    read_method!(grid_export(sheet_name: &str, range: &str) -> GridPayload);
    pub fn range_values(
        &self,
        sheet: &str,
        ranges: impl Into<SessionRangeSelection>,
    ) -> Result<Vec<RangeValuesEntry>> {
        self.read(|document| document.range_values(sheet, ranges))
    }
    pub fn capture(&self) -> Result<(String, Vec<u8>)> {
        self.capture_with_coverage()
            .map(|(revision, bytes, _)| (revision, bytes))
    }
    pub fn capture_with_coverage(&self) -> Result<(String, Vec<u8>, EvaluationCoverage)> {
        let _gate = self
            .0
            .gate
            .try_lock()
            .ok_or_else(|| anyhow!("session is busy"))?;
        let mut owner = self
            .0
            .runtime
            .try_borrow_mut()
            .map_err(|_| anyhow!("session is busy"))?;
        let owner = owner
            .as_mut()
            .ok_or_else(|| anyhow!("resource has been discarded"))?;
        let coverage = owner.owner.read_view()?.imported_evaluation_coverage();
        Ok((
            owner.owner.revision(),
            owner.owner.diagnostic_workbook().snapshot_bytes()?,
            coverage,
        ))
    }
    pub async fn execute(
        &self,
        request_id: &str,
        operation: SpreadsheetOperation,
    ) -> Result<CanonicalResponse, CanonicalErrorEnvelope> {
        self.execute_with_baseline(request_id, operation, None)
            .await
    }
    pub async fn execute_with_baseline(
        &self,
        request_id: &str,
        operation: SpreadsheetOperation,
        baseline: Option<(
            ResourceId,
            agent_spreadsheet::canonical_lifecycle::VerificationSnapshot,
        )>,
    ) -> Result<CanonicalResponse, CanonicalErrorEnvelope> {
        let _gate = self.0.gate.lock().await;
        let mut owner = self.0.runtime.borrow_mut();
        let Some(runtime) = owner.as_mut() else {
            return execute_terminal_session(
                &self.0.storage,
                &self.0.resource,
                request_id,
                operation,
            )
            .await;
        };
        let result = runtime
            .execute_with_verification_baseline(request_id, operation, baseline)
            .await;
        if runtime.is_discarded() {
            *owner = None;
        }
        result
    }
    fn legacy_write(&self, ops: Vec<Value>, preview: bool) -> Result<CanonicalResponse> {
        let revision = self.revision()?;
        let operation = decode_operation(
            "write",
            json!({
                "resource_id": self.0.resource, "expected_revision": revision,
                "mode": if preview { "preview" } else { "apply" }, "atomic": true, "ops": ops,
            }),
        )
        .map_err(|error| anyhow!(error.error.message))?;
        // This path is deliberately restricted to write + MemoryJournal: neither
        // performs host I/O or yields. Backed hosts use the asynchronous entrypoint.
        let request_id = agent_spreadsheet::utils::make_short_random_id("compat", 24);
        let response = self
            .execute(&request_id, operation)
            .now_or_never()
            .ok_or_else(|| anyhow!("session is busy; use the asynchronous operation API"))?
            .map_err(|error| anyhow!(error.error.message))?;
        if !matches!(
            response.data["status"].as_str(),
            Some("applied" | "previewed")
        ) {
            bail!("write did not complete: {}", response.data);
        }
        Ok(response)
    }
    fn name_write<T: DeserializeOwned>(&self, op: Value) -> Result<T> {
        let response = self.legacy_write(vec![op], false)?;
        Ok(serde_json::from_value(
            response.data["results"][0]["detail"]["name_result"].clone(),
        )?)
    }
    pub fn define_name(
        &self,
        name: &str,
        refers_to: &str,
        scope: Option<&str>,
        scope_sheet_name: Option<&str>,
    ) -> Result<DefineNameResponse> {
        self.name_write(json!({"kind":"define_name", "name":name, "refers_to":refers_to, "scope":scope.unwrap_or("workbook"), "scope_sheet_name":scope_sheet_name}))
    }
    pub fn update_name(
        &self,
        name: &str,
        refers_to: Option<&str>,
        scope: Option<&str>,
        scope_sheet_name: Option<&str>,
    ) -> Result<UpdateNameResponse> {
        self.name_write(json!({"kind":"update_name", "name":name, "refers_to":refers_to, "scope":scope, "scope_sheet_name":scope_sheet_name}))
    }
    pub fn delete_name(
        &self,
        name: &str,
        scope: Option<&str>,
        scope_sheet_name: Option<&str>,
    ) -> Result<DeleteNameResponse> {
        self.name_write(json!({"kind":"delete_name", "name":name, "scope":scope, "scope_sheet_name":scope_sheet_name}))
    }
    pub fn apply_ops(
        &self,
        ops: &[SessionTransformOp],
        preview: bool,
    ) -> Result<SessionApplySummary> {
        let response = self.legacy_write(
            ops.iter()
                .map(serde_json::to_value)
                .collect::<Result<_, _>>()?,
            preview,
        )?;
        let results = response.data["results"]
            .as_array()
            .ok_or_else(|| anyhow!("write results missing"))?;
        let count = |key: &str| {
            results
                .iter()
                .map(|result| result["detail"]["counts"][key].as_u64().unwrap_or(0))
                .sum()
        };
        Ok(SessionApplySummary {
            ops_applied: results.len(),
            cells_touched: count("cells_touched"),
            cells_value_set: count("cells_value_set"),
            cells_formula_set: count("cells_formula_set"),
            cells_formula_cleared: count("cells_formula_cleared"),
            cells_skipped_keep_formulas: count("cells_skipped_keep_formulas"),
        })
    }
}
