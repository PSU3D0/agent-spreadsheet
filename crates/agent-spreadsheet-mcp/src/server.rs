use crate::canonical_router;
use crate::config::ServerConfig;
use crate::errors::InvalidParamsError;
use crate::model::{
    CloseWorkbookResponse, FindFormulaResponse, FindValueResponse, FormulaTraceResponse,
    InspectCellsResponse, LayoutPageResponse, ManifestStubResponse, NamedRangesResponse,
    RangeValuesResponse, ReadTableResponse, SheetFormulaMapResponse, SheetListResponse,
    SheetOverviewResponse, SheetPageResponse, SheetStatisticsResponse, SheetStylesResponse,
    TableProfileResponse, VolatileScanResponse, WorkbookDescription, WorkbookListResponse,
    WorkbookStyleSummaryResponse, WorkbookSummaryResponse,
};
#[cfg(feature = "recalc")]
use crate::model::{DefineNameResponse, DeleteNameResponse, UpdateNameResponse};
use crate::response_prune::Pruned;
#[cfg(feature = "recalc")]
use crate::response_prune::to_pruned_value;
use crate::state::AppState;
use crate::tools;
use anyhow::{Result, anyhow};
use rmcp::{
    ErrorData as McpError, Json as McpJson, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::Serialize;
use serde_json::Value;
use std::future::Future;
use std::sync::Arc;
use thiserror::Error;
use {once_cell::sync::Lazy, regex::Regex};

type Json<T> = McpJson<Pruned<T>>;

fn json<T>(value: T) -> Json<T> {
    McpJson(Pruned(value))
}

const CANONICAL_INSTRUCTIONS: &str = "Spreadsheet MCP exposes canonical, versioned spreadsheet operations. Start with list_workbooks, then use resource_id and revision_id from canonical envelopes. Reads are bounded; use cursors to continue. For edits, create_fork, write with revision CAS, recalculate, verify_workbook, inspect get_changes, then export_fork or discard_fork. Risk annotations are worst-case hints; read each tool description and use preview/stage/checkpoint actions before destructive changes. Optional tools/call _meta[\"agent-spreadsheet/request-id\"] (1-256 bytes) enables identical retries. Otherwise each call gets a fresh ID in result _meta; lost responses are not safe to retry blindly.";

fn build_instructions(_recalc_enabled: bool, _vba_enabled: bool) -> String {
    CANONICAL_INSTRUCTIONS.to_string()
}

#[derive(Clone)]
pub struct SpreadsheetServer {
    state: Arc<AppState>,
    tool_router: SlimToolRouter,
}

/// Wrapper around [`ToolRouter`] whose `list_all()` omits `outputSchema`.
///
/// Output schemas accounted for ~57% of the `tools/list` payload (100KB+ of
/// ~178KB compact) while contributing nothing to tool *selection*: callers
/// read actual responses (`structuredContent`), which remain untouched. The
/// byte budget belongs to input schemas and descriptions.
#[derive(Clone)]
struct SlimToolRouter(ToolRouter<SpreadsheetServer>);

impl SlimToolRouter {
    fn list_all(&self) -> Vec<rmcp::model::Tool> {
        self.0
            .list_all()
            .into_iter()
            .map(|mut tool| {
                tool.output_schema = None;
                tool
            })
            .collect()
    }

    async fn call(
        &self,
        context: rmcp::handler::server::tool::ToolCallContext<'_, SpreadsheetServer>,
    ) -> Result<rmcp::model::CallToolResult, McpError> {
        self.0.call(context).await
    }
}

impl SpreadsheetServer {
    pub async fn new(config: Arc<ServerConfig>) -> Result<Self> {
        config.ensure_workspace_root()?;
        let state = Arc::new(AppState::new(config));
        Ok(Self::from_state(state))
    }

    pub fn from_state(state: Arc<AppState>) -> Self {
        let capabilities = canonical_router::runtime_capabilities(&state);
        let mut canonical = canonical_router::canonical_tool_router(&capabilities);

        if !state.config().slim_surface {
            let mut legacy = Self::tool_router();
            #[cfg(feature = "recalc")]
            {
                legacy.merge(Self::fork_tool_router());
                legacy.merge(Self::legacy_write_tool_router());
            }
            if state.config().vba_enabled {
                legacy.merge(Self::vba_tool_router());
            }

            let legacy_names = legacy.map.keys().cloned().collect::<Vec<_>>();
            for name in legacy_names {
                canonical.remove_route(&name);
            }
            canonical.merge(legacy);
        }

        Self {
            state,
            tool_router: SlimToolRouter(canonical),
        }
    }

    /// Names of all tools currently registered in the router (post feature /
    /// slim-surface filtering). Exposed for tests and diagnostics.
    pub fn tool_names(&self) -> Vec<String> {
        self.tool_router
            .list_all()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect()
    }

    pub async fn run_stdio(self) -> Result<()> {
        let service = self
            .serve(stdio())
            .await
            .inspect_err(|error| tracing::error!("serving error: {:?}", error))?;
        service.waiting().await?;
        Ok(())
    }

    pub async fn run(self) -> Result<()> {
        self.run_stdio().await
    }

    fn ensure_tool_enabled(&self, tool: &str) -> Result<()> {
        tracing::info!(tool = tool, "tool invocation requested");
        if self.state.config().is_tool_enabled(tool) {
            Ok(())
        } else {
            Err(ToolDisabledError::new(tool).into())
        }
    }

    pub(crate) fn canonical_state(&self) -> Arc<AppState> {
        self.state.clone()
    }

    pub(crate) fn canonical_tool_timeout(&self) -> Option<std::time::Duration> {
        self.state.config().tool_timeout()
    }

    /// Adapter response-size ceiling, if configured.
    pub(crate) fn canonical_response_limit(&self) -> Option<usize> {
        self.state.config().max_response_bytes()
    }

    /// Whether the operation passes the configured enabled-tool policy.
    pub(crate) fn canonical_tool_allowed(&self, tool: &str) -> bool {
        self.state.config().is_tool_enabled(tool)
    }

    pub(crate) fn ensure_canonical_tool_enabled(&self, tool: &str) -> Result<(), McpError> {
        self.ensure_tool_enabled(tool)
            .map_err(|error| to_mcp_error_for_tool(tool, error))
    }

    pub(crate) fn ensure_canonical_response_size<T: Serialize>(
        &self,
        tool: &str,
        value: &T,
    ) -> Result<(), McpError> {
        self.ensure_response_size(tool, value).map_err(|error| {
            // This check runs after canonical execution. Do not advise changing
            // and retrying a mutation whose effects may already be committed.
            let message = if let Some(size) = error.downcast_ref::<ResponseTooLargeError>() {
                format!("response is {} bytes, exceeding the {} byte adapter limit", size.size, size.limit)
            } else {
                error.to_string()
            };
            McpError::internal_error(message, None)
        })
    }

    pub(crate) async fn execute_canonical_operation(
        &self,
        operation: &'static str,
        arguments: Value,
        request_id: Option<String>,
    ) -> Result<rmcp::model::CallToolResult, McpError> {
        canonical_router::execute(self, operation, arguments, request_id).await
    }

    fn ensure_vba_enabled(&self, tool: &str) -> Result<()> {
        self.ensure_tool_enabled(tool)?;
        if self.state.config().vba_enabled {
            Ok(())
        } else {
            Err(VbaDisabledError.into())
        }
    }

    #[cfg(feature = "recalc")]
    fn ensure_recalc_enabled(&self, tool: &str) -> Result<()> {
        self.ensure_tool_enabled(tool)?;
        if self.state.config().recalc_enabled {
            Ok(())
        } else {
            Err(RecalcDisabledError.into())
        }
    }

    async fn run_tool_with_timeout<T, F>(&self, tool: &str, fut: F) -> Result<T>
    where
        F: Future<Output = Result<T>>,
        T: Serialize,
    {
        let result = if let Some(timeout_duration) = self.state.config().tool_timeout() {
            match tokio::time::timeout(timeout_duration, fut).await {
                Ok(result) => result,
                Err(_) => Err(anyhow!(
                    "tool '{}' timed out after {}ms",
                    tool,
                    timeout_duration.as_millis()
                )),
            }
        } else {
            fut.await
        }?;

        self.ensure_response_size(tool, &result)?;
        Ok(result)
    }

    fn ensure_response_size<T: Serialize>(&self, tool: &str, value: &T) -> Result<()> {
        let Some(limit) = self.state.config().max_response_bytes() else {
            return Ok(());
        };
        let payload = serde_json::to_vec(value)
            .map_err(|e| anyhow!("failed to serialize response for {}: {}", tool, e))?;
        if payload.len() > limit {
            return Err(ResponseTooLargeError::new(tool, payload.len(), limit).into());
        }
        Ok(())
    }
}

#[tool_router]
impl SpreadsheetServer {
    #[tool(
        name = "list_workbooks",
        description = "List spreadsheet files in the workspace"
    )]
    pub async fn list_workbooks(
        &self,
        Parameters(params): Parameters<tools::ListWorkbooksParams>,
    ) -> Result<Json<WorkbookListResponse>, McpError> {
        self.ensure_tool_enabled("list_workbooks")
            .map_err(|e| to_mcp_error_for_tool("list_workbooks", e))?;
        self.run_tool_with_timeout(
            "list_workbooks",
            tools::list_workbooks(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("list_workbooks", e))
    }

    #[tool(name = "describe_workbook", description = "Describe workbook metadata")]
    pub async fn describe_workbook(
        &self,
        Parameters(params): Parameters<tools::DescribeWorkbookParams>,
    ) -> Result<Json<WorkbookDescription>, McpError> {
        self.ensure_tool_enabled("describe_workbook")
            .map_err(|e| to_mcp_error_for_tool("describe_workbook", e))?;
        self.run_tool_with_timeout(
            "describe_workbook",
            tools::describe_workbook(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("describe_workbook", e))
    }

    #[tool(
        name = "workbook_summary",
        description = "Summarize workbook regions and entry points"
    )]
    pub async fn workbook_summary(
        &self,
        Parameters(params): Parameters<tools::WorkbookSummaryParams>,
    ) -> Result<Json<WorkbookSummaryResponse>, McpError> {
        self.ensure_tool_enabled("workbook_summary")
            .map_err(|e| to_mcp_error_for_tool("workbook_summary", e))?;
        self.run_tool_with_timeout(
            "workbook_summary",
            tools::workbook_summary(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("workbook_summary", e))
    }

    #[tool(name = "list_sheets", description = "List sheets with summaries")]
    pub async fn list_sheets(
        &self,
        Parameters(params): Parameters<tools::ListSheetsParams>,
    ) -> Result<Json<SheetListResponse>, McpError> {
        self.ensure_tool_enabled("list_sheets")
            .map_err(|e| to_mcp_error_for_tool("list_sheets", e))?;
        self.run_tool_with_timeout(
            "list_sheets",
            tools::list_sheets(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("list_sheets", e))
    }

    #[tool(
        name = "sheet_overview",
        description = "Get narrative overview for a sheet"
    )]
    pub async fn sheet_overview(
        &self,
        Parameters(params): Parameters<tools::SheetOverviewParams>,
    ) -> Result<Json<SheetOverviewResponse>, McpError> {
        self.ensure_tool_enabled("sheet_overview")
            .map_err(|e| to_mcp_error_for_tool("sheet_overview", e))?;
        self.run_tool_with_timeout(
            "sheet_overview",
            tools::sheet_overview(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("sheet_overview", e))
    }

    #[tool(
        name = "sheet_page",
        description = "Raw paged cell dump. Use for unstructured sheets or when region detection fails; prefer read_table for tabular data. Works on forks (recalculate first or values are stale). Truncated responses include a budget object with continuation hints."
    )]
    pub async fn sheet_page(
        &self,
        Parameters(params): Parameters<tools::SheetPageParams>,
    ) -> Result<Json<SheetPageResponse>, McpError> {
        self.ensure_tool_enabled("sheet_page")
            .map_err(|e| to_mcp_error_for_tool("sheet_page", e))?;
        self.run_tool_with_timeout("sheet_page", tools::sheet_page(self.state.clone(), params))
            .await
            .map(json)
            .map_err(|e| to_mcp_error_for_tool("sheet_page", e))
    }

    #[tool(name = "find_value", description = "Search cell values or labels")]
    pub async fn find_value(
        &self,
        Parameters(params): Parameters<tools::FindValueParams>,
    ) -> Result<Json<FindValueResponse>, McpError> {
        self.ensure_tool_enabled("find_value")
            .map_err(|e| to_mcp_error_for_tool("find_value", e))?;
        self.run_tool_with_timeout("find_value", tools::find_value(self.state.clone(), params))
            .await
            .map(json)
            .map_err(|e| to_mcp_error_for_tool("find_value", e))
    }

    #[tool(
        name = "read_table",
        description = "Structured table extraction (headers, filters, sampling) — the go-to read for tabular data. Prefer region_id from sheet_overview or a tight range; defaults to format=csv with limit/sample_mode for large tables. Use range_values for raw spot reads."
    )]
    pub async fn read_table(
        &self,
        Parameters(params): Parameters<tools::ReadTableParams>,
    ) -> Result<Json<ReadTableResponse>, McpError> {
        self.ensure_tool_enabled("read_table")
            .map_err(|e| to_mcp_error_for_tool("read_table", e))?;
        self.run_tool_with_timeout("read_table", tools::read_table(self.state.clone(), params))
            .await
            .map(json)
            .map_err(|e| to_mcp_error_for_tool("read_table", e))
    }

    #[tool(name = "table_profile", description = "Profile a region or table")]
    pub async fn table_profile(
        &self,
        Parameters(params): Parameters<tools::TableProfileParams>,
    ) -> Result<Json<TableProfileResponse>, McpError> {
        self.ensure_tool_enabled("table_profile")
            .map_err(|e| to_mcp_error_for_tool("table_profile", e))?;
        self.run_tool_with_timeout(
            "table_profile",
            tools::table_profile(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("table_profile", e))
    }

    #[tool(
        name = "range_values",
        description = "Fetch raw values for specific A1 ranges — the default way to read computed results, including from a fork after recalculate. Defaults to format=values; csv/json available. Prefer read_table for header-aware tabular extraction."
    )]
    pub async fn range_values(
        &self,
        Parameters(params): Parameters<tools::RangeValuesParams>,
    ) -> Result<Json<RangeValuesResponse>, McpError> {
        self.ensure_tool_enabled("range_values")
            .map_err(|e| to_mcp_error_for_tool("range_values", e))?;
        self.run_tool_with_timeout(
            "range_values",
            tools::range_values(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("range_values", e))
    }

    #[tool(
        name = "inspect_cells",
        description = "Deep per-cell detail (value, formula, style, number format) for up to 25 addresses. NOT for bulk reads — use sheet_page or range_values. Use to spot-check cells found via sheet_overview/find_value; on a fork, recalculate first for fresh values."
    )]
    pub async fn inspect_cells(
        &self,
        Parameters(params): Parameters<tools::InspectCellsParams>,
    ) -> Result<Json<InspectCellsResponse>, McpError> {
        self.ensure_tool_enabled("inspect_cells")
            .map_err(|e| to_mcp_error_for_tool("inspect_cells", e))?;
        self.run_tool_with_timeout(
            "inspect_cells",
            tools::inspect_cells(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("inspect_cells", e))
    }

    #[tool(
        name = "sheet_statistics",
        description = "Get aggregated sheet statistics"
    )]
    pub async fn sheet_statistics(
        &self,
        Parameters(params): Parameters<tools::SheetStatisticsParams>,
    ) -> Result<Json<SheetStatisticsResponse>, McpError> {
        self.ensure_tool_enabled("sheet_statistics")
            .map_err(|e| to_mcp_error_for_tool("sheet_statistics", e))?;
        self.run_tool_with_timeout(
            "sheet_statistics",
            tools::sheet_statistics(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("sheet_statistics", e))
    }

    #[tool(
        name = "sheet_formula_map",
        description = "Summarize formula groups across a sheet"
    )]
    pub async fn sheet_formula_map(
        &self,
        Parameters(params): Parameters<tools::SheetFormulaMapParams>,
    ) -> Result<Json<SheetFormulaMapResponse>, McpError> {
        self.ensure_tool_enabled("sheet_formula_map")
            .map_err(|e| to_mcp_error_for_tool("sheet_formula_map", e))?;
        self.run_tool_with_timeout(
            "sheet_formula_map",
            tools::sheet_formula_map(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("sheet_formula_map", e))
    }

    #[tool(
        name = "formula_trace",
        description = "Trace formula precedents or dependents"
    )]
    pub async fn formula_trace(
        &self,
        Parameters(params): Parameters<tools::FormulaTraceParams>,
    ) -> Result<Json<FormulaTraceResponse>, McpError> {
        self.ensure_tool_enabled("formula_trace")
            .map_err(|e| to_mcp_error_for_tool("formula_trace", e))?;
        self.run_tool_with_timeout(
            "formula_trace",
            tools::formula_trace(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("formula_trace", e))
    }

    #[tool(name = "named_ranges", description = "List named ranges and tables")]
    pub async fn named_ranges(
        &self,
        Parameters(params): Parameters<tools::NamedRangesParams>,
    ) -> Result<Json<NamedRangesResponse>, McpError> {
        self.ensure_tool_enabled("named_ranges")
            .map_err(|e| to_mcp_error_for_tool("named_ranges", e))?;
        self.run_tool_with_timeout(
            "named_ranges",
            tools::named_ranges(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("named_ranges", e))
    }

    #[tool(
        name = "verify_workbook",
        description = "Compares two workbook/fork states (baseline vs current) reporting target proof plus new/resolved/preexisting errors. Only meaningful when the current side has been recalculated; unevaluated formulas have empty caches and diff as clean."
    )]
    pub async fn verify_workbook(
        &self,
        Parameters(params): Parameters<tools::VerifyWorkbookParams>,
    ) -> Result<Json<agent_spreadsheet::verification::VerifyResponse>, McpError> {
        self.ensure_tool_enabled("verify_workbook")
            .map_err(|e| to_mcp_error_for_tool("verify_workbook", e))?;
        self.run_tool_with_timeout(
            "verify_workbook",
            tools::verify_workbook(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("verify_workbook", e))
    }

    #[tool(
        name = "find_formula",
        description = "Search formulas containing text. Defaults: include_context=false, limit=50; use offset for paging."
    )]
    pub async fn find_formula(
        &self,
        Parameters(params): Parameters<tools::FindFormulaParams>,
    ) -> Result<Json<FindFormulaResponse>, McpError> {
        self.ensure_tool_enabled("find_formula")
            .map_err(|e| to_mcp_error_for_tool("find_formula", e))?;
        self.run_tool_with_timeout(
            "find_formula",
            tools::find_formula(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("find_formula", e))
    }

    #[tool(name = "scan_volatiles", description = "Scan for volatile formulas")]
    pub async fn scan_volatiles(
        &self,
        Parameters(params): Parameters<tools::ScanVolatilesParams>,
    ) -> Result<Json<VolatileScanResponse>, McpError> {
        self.ensure_tool_enabled("scan_volatiles")
            .map_err(|e| to_mcp_error_for_tool("scan_volatiles", e))?;
        self.run_tool_with_timeout(
            "scan_volatiles",
            tools::scan_volatiles(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("scan_volatiles", e))
    }

    #[tool(
        name = "sheet_styles",
        description = "Summarise style usage and properties for a sheet"
    )]
    pub async fn sheet_styles(
        &self,
        Parameters(params): Parameters<tools::SheetStylesParams>,
    ) -> Result<Json<SheetStylesResponse>, McpError> {
        self.ensure_tool_enabled("sheet_styles")
            .map_err(|e| to_mcp_error_for_tool("sheet_styles", e))?;
        self.run_tool_with_timeout(
            "sheet_styles",
            tools::sheet_styles(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("sheet_styles", e))
    }

    #[tool(
        name = "layout_page",
        description = "Render a sheet range with layout semantics: column widths, borders, bold/italic, alignment, and merged cells. Returns a JSON layout plane (per-column widths, per-cell style metadata) and optionally an ASCII grid render. Use render=ascii or render=both to include the ASCII view. Capped at 80 rows × 25 columns."
    )]
    pub async fn layout_page(
        &self,
        Parameters(params): Parameters<tools::LayoutPageParams>,
    ) -> Result<Json<LayoutPageResponse>, McpError> {
        self.ensure_tool_enabled("layout_page")
            .map_err(|e| to_mcp_error_for_tool("layout_page", e))?;
        self.run_tool_with_timeout(
            "layout_page",
            tools::layout_page(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("layout_page", e))
    }

    #[tool(
        name = "grid_export",
        description = "Export a range into a rich grid payload containing per-cell values, formulas, number formats, styles, column sizes, and merges. Returns inline JSON."
    )]
    pub async fn grid_export(
        &self,
        Parameters(params): Parameters<tools::GridExportParams>,
    ) -> Result<Json<agent_spreadsheet::model::GridPayload>, McpError> {
        self.ensure_tool_enabled("grid_export")
            .map_err(|e| to_mcp_error_for_tool("grid_export", e))?;
        self.run_tool_with_timeout(
            "grid_export",
            tools::grid_export(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("grid_export", e))
    }

    #[tool(
        name = "workbook_style_summary",
        description = "Summarise style usage, theme colors, and conditional formats across a workbook"
    )]
    pub async fn workbook_style_summary(
        &self,
        Parameters(params): Parameters<tools::WorkbookStyleSummaryParams>,
    ) -> Result<Json<WorkbookStyleSummaryResponse>, McpError> {
        self.ensure_tool_enabled("workbook_style_summary")
            .map_err(|e| to_mcp_error_for_tool("workbook_style_summary", e))?;
        self.run_tool_with_timeout(
            "workbook_style_summary",
            tools::workbook_style_summary(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("workbook_style_summary", e))
    }

    #[tool(
        name = "get_manifest_stub",
        description = "Generate manifest scaffold for workbook"
    )]
    pub async fn get_manifest_stub(
        &self,
        Parameters(params): Parameters<tools::ManifestStubParams>,
    ) -> Result<Json<ManifestStubResponse>, McpError> {
        self.ensure_tool_enabled("get_manifest_stub")
            .map_err(|e| to_mcp_error_for_tool("get_manifest_stub", e))?;
        self.run_tool_with_timeout(
            "get_manifest_stub",
            tools::get_manifest_stub(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("get_manifest_stub", e))
    }

    #[tool(
        name = "execute_manifest",
        description = "Execute a SheetPort manifest with JSON inputs"
    )]
    pub async fn execute_manifest(
        &self,
        Parameters(params): Parameters<tools::ExecuteManifestParams>,
    ) -> Result<Json<tools::ExecuteManifestResponse>, McpError> {
        self.ensure_tool_enabled("execute_manifest")
            .map_err(|e| to_mcp_error_for_tool("execute_manifest", e))?;
        self.run_tool_with_timeout(
            "execute_manifest",
            tools::execute_manifest(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("execute_manifest", e))
    }

    #[tool(name = "close_workbook", description = "Evict a workbook from cache")]
    pub async fn close_workbook(
        &self,
        Parameters(params): Parameters<tools::CloseWorkbookParams>,
    ) -> Result<Json<CloseWorkbookResponse>, McpError> {
        self.ensure_tool_enabled("close_workbook")
            .map_err(|e| to_mcp_error_for_tool("close_workbook", e))?;
        self.run_tool_with_timeout(
            "close_workbook",
            tools::close_workbook(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("close_workbook", e))
    }
}

#[tool_router(router = vba_tool_router)]
impl SpreadsheetServer {
    #[tool(
        name = "vba_project_summary",
        description = "Summarize embedded VBA project (xlsm)"
    )]
    pub async fn vba_project_summary(
        &self,
        Parameters(params): Parameters<tools::vba::VbaProjectSummaryParams>,
    ) -> Result<Json<crate::model::VbaProjectSummaryResponse>, McpError> {
        self.ensure_vba_enabled("vba_project_summary")
            .map_err(|e| to_mcp_error_for_tool("vba_project_summary", e))?;
        self.run_tool_with_timeout(
            "vba_project_summary",
            tools::vba::vba_project_summary(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("vba_project_summary", e))
    }

    #[tool(
        name = "vba_module_source",
        description = "Read VBA module source (paged)"
    )]
    pub async fn vba_module_source(
        &self,
        Parameters(params): Parameters<tools::vba::VbaModuleSourceParams>,
    ) -> Result<Json<crate::model::VbaModuleSourceResponse>, McpError> {
        self.ensure_vba_enabled("vba_module_source")
            .map_err(|e| to_mcp_error_for_tool("vba_module_source", e))?;
        self.run_tool_with_timeout(
            "vba_module_source",
            tools::vba::vba_module_source(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("vba_module_source", e))
    }
}

#[cfg(feature = "recalc")]
#[tool_router(router = fork_tool_router)]
impl SpreadsheetServer {
    #[tool(
        name = "create_fork",
        description = "Start of every write workflow: creates an editable copy of a workbook and returns fork_id; the original file is never modified. Pass fork_id to mutate_batch/edit_batch/recalculate/get_changeset/save_fork. Only .xlsx is supported."
    )]
    pub async fn create_fork(
        &self,
        Parameters(params): Parameters<tools::fork::CreateForkParams>,
    ) -> Result<Json<tools::fork::CreateForkResponse>, McpError> {
        self.ensure_recalc_enabled("create_fork")
            .map_err(|e| to_mcp_error_for_tool("create_fork", e))?;
        self.run_tool_with_timeout(
            "create_fork",
            tools::fork::create_fork(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("create_fork", e))
    }

    #[tool(
        name = "edit_batch",
        description = "Per-cell value/formula edits on a fork: {fork_id, sheet_name, edits:[`A1=100` | {address,value,is_formula}]}. mode=preview stages instead of applying. For bulk fills/clears/structure prefer mutate_batch. Run recalculate before reading computed values."
    )]
    pub async fn edit_batch(
        &self,
        Parameters(params): Parameters<tools::write_normalize::EditBatchParamsInput>,
    ) -> Result<Json<tools::fork::EditBatchResponse>, McpError> {
        self.ensure_recalc_enabled("edit_batch")
            .map_err(|e| to_mcp_error_for_tool("edit_batch", e))?;
        self.run_tool_with_timeout(
            "edit_batch",
            tools::fork::edit_batch(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("edit_batch", e))
    }

    #[tool(
        name = "mutate_batch",
        description = "Unified fork write tool: ops is a batch of tagged ops (fill_range/clear_range/write_matrix, insert_rows/delete_cols/create_sheet, style, set_data_validation, freeze_panes, column_size, formula_pattern, replace_in_formulas, ...). mode=preview stages without mutating (returns ops_staged); mode=apply mutates. Recalculate after applying."
    )]
    pub async fn mutate_batch(
        &self,
        Parameters(params): Parameters<tools::mutate_batch::MutateBatchParams>,
    ) -> Result<Json<tools::mutate_batch::MutateBatchResponse>, McpError> {
        self.ensure_recalc_enabled("mutate_batch")
            .map_err(|e| to_mcp_error_for_tool("mutate_batch", e))?;
        self.run_tool_with_timeout(
            "mutate_batch",
            tools::mutate_batch::mutate_batch(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("mutate_batch", e))
    }

    #[tool(
        name = "grid_import",
        description = "Import a rich grid payload containing values, formulas, styles, formats, column sizes, and merges."
    )]
    pub async fn grid_import(
        &self,
        Parameters(params): Parameters<tools::fork::GridImportParams>,
    ) -> Result<Json<tools::fork::GridImportResponse>, McpError> {
        self.ensure_recalc_enabled("grid_import")
            .map_err(|e| to_mcp_error_for_tool("grid_import", e))?;
        self.run_tool_with_timeout(
            "grid_import",
            tools::fork::grid_import(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("grid_import", e))
    }

    #[tool(
        name = "define_name",
        description = "Define a new named range in a fork. Scope: 'workbook' (default) or 'sheet'. \
Requires scope_sheet_name when scope is 'sheet'."
    )]
    pub async fn define_name(
        &self,
        Parameters(params): Parameters<tools::DefineNameParams>,
    ) -> Result<Json<DefineNameResponse>, McpError> {
        self.ensure_recalc_enabled("define_name")
            .map_err(|e| to_mcp_error_for_tool("define_name", e))?;
        self.run_tool_with_timeout(
            "define_name",
            tools::define_name(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("define_name", e))
    }

    #[tool(
        name = "update_name",
        description = "Update an existing named range's refers_to in a fork. \
Scope filter: 'workbook' or 'sheet' to disambiguate."
    )]
    pub async fn update_name(
        &self,
        Parameters(params): Parameters<tools::UpdateNameParams>,
    ) -> Result<Json<UpdateNameResponse>, McpError> {
        self.ensure_recalc_enabled("update_name")
            .map_err(|e| to_mcp_error_for_tool("update_name", e))?;
        self.run_tool_with_timeout(
            "update_name",
            tools::update_name(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("update_name", e))
    }

    #[tool(
        name = "delete_name",
        description = "Delete a named range from a fork. \
Scope filter: 'workbook' or 'sheet' to disambiguate."
    )]
    pub async fn delete_name(
        &self,
        Parameters(params): Parameters<tools::DeleteNameParams>,
    ) -> Result<Json<DeleteNameResponse>, McpError> {
        self.ensure_recalc_enabled("delete_name")
            .map_err(|e| to_mcp_error_for_tool("delete_name", e))?;
        self.run_tool_with_timeout(
            "delete_name",
            tools::delete_name(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("delete_name", e))
    }

    #[tool(name = "get_edits", description = "List all edits applied to a fork")]
    pub async fn get_edits(
        &self,
        Parameters(params): Parameters<tools::fork::GetEditsParams>,
    ) -> Result<Json<tools::fork::GetEditsResponse>, McpError> {
        self.ensure_recalc_enabled("get_edits")
            .map_err(|e| to_mcp_error_for_tool("get_edits", e))?;
        self.run_tool_with_timeout(
            "get_edits",
            tools::fork::get_edits(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("get_edits", e))
    }

    #[tool(
        name = "get_changeset",
        description = "Calculate diff between fork and base workbook. Defaults: limit=200. Supports limit/offset paging and type/subtype filters; returns summary."
    )]
    pub async fn get_changeset(
        &self,
        Parameters(params): Parameters<tools::fork::GetChangesetParams>,
    ) -> Result<Json<tools::fork::GetChangesetResponse>, McpError> {
        self.ensure_recalc_enabled("get_changeset")
            .map_err(|e| to_mcp_error_for_tool("get_changeset", e))?;
        self.run_tool_with_timeout(
            "get_changeset",
            tools::fork::get_changeset(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("get_changeset", e))
    }

    #[tool(
        name = "recalculate",
        description = "Recompute all formulas in a fork. Required after any edit before reading computed values; reads on a dirty fork return stale caches. Also required before verify_workbook or get_changeset can prove anything. May take seconds on complex workbooks."
    )]
    pub async fn recalculate(
        &self,
        Parameters(params): Parameters<tools::fork::RecalculateParams>,
    ) -> Result<Json<tools::fork::RecalculateResponse>, McpError> {
        self.ensure_recalc_enabled("recalculate")
            .map_err(|e| to_mcp_error_for_tool("recalculate", e))?;
        self.run_tool_with_timeout(
            "recalculate",
            tools::fork::recalculate(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("recalculate", e))
    }

    #[tool(name = "list_forks", description = "List all active forks")]
    pub async fn list_forks(
        &self,
        Parameters(params): Parameters<tools::fork::ListForksParams>,
    ) -> Result<Json<tools::fork::ListForksResponse>, McpError> {
        self.ensure_recalc_enabled("list_forks")
            .map_err(|e| to_mcp_error_for_tool("list_forks", e))?;
        self.run_tool_with_timeout(
            "list_forks",
            tools::fork::list_forks(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("list_forks", e))
    }

    #[tool(name = "discard_fork", description = "Discard a fork without saving")]
    pub async fn discard_fork(
        &self,
        Parameters(params): Parameters<tools::fork::DiscardForkParams>,
    ) -> Result<Json<tools::fork::DiscardForkResponse>, McpError> {
        self.ensure_recalc_enabled("discard_fork")
            .map_err(|e| to_mcp_error_for_tool("discard_fork", e))?;
        self.run_tool_with_timeout(
            "discard_fork",
            tools::fork::discard_fork(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("discard_fork", e))
    }

    #[tool(
        name = "save_fork",
        description = "Exports the fork to a file on disk (target_path) and discards the fork by default (drop_fork=false keeps it). NOT for reading results — use range_values on the fork instead. Overwriting the original requires server --allow-overwrite."
    )]
    pub async fn save_fork(
        &self,
        Parameters(params): Parameters<tools::fork::SaveForkParams>,
    ) -> Result<Json<tools::fork::SaveForkResponse>, McpError> {
        self.ensure_recalc_enabled("save_fork")
            .map_err(|e| to_mcp_error_for_tool("save_fork", e))?;
        self.run_tool_with_timeout(
            "save_fork",
            tools::fork::save_fork(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("save_fork", e))
    }

    #[tool(
        name = "checkpoint_fork",
        description = "Create a high-fidelity checkpoint snapshot of a fork"
    )]
    pub async fn checkpoint_fork(
        &self,
        Parameters(params): Parameters<tools::fork::CheckpointForkParams>,
    ) -> Result<Json<tools::fork::CheckpointForkResponse>, McpError> {
        self.ensure_recalc_enabled("checkpoint_fork")
            .map_err(|e| to_mcp_error_for_tool("checkpoint_fork", e))?;
        self.run_tool_with_timeout(
            "checkpoint_fork",
            tools::fork::checkpoint_fork(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("checkpoint_fork", e))
    }

    #[tool(name = "list_checkpoints", description = "List checkpoints for a fork")]
    pub async fn list_checkpoints(
        &self,
        Parameters(params): Parameters<tools::fork::ListCheckpointsParams>,
    ) -> Result<Json<tools::fork::ListCheckpointsResponse>, McpError> {
        self.ensure_recalc_enabled("list_checkpoints")
            .map_err(|e| to_mcp_error_for_tool("list_checkpoints", e))?;
        self.run_tool_with_timeout(
            "list_checkpoints",
            tools::fork::list_checkpoints(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("list_checkpoints", e))
    }

    #[tool(
        name = "restore_checkpoint",
        description = "Restore a fork to a checkpoint"
    )]
    pub async fn restore_checkpoint(
        &self,
        Parameters(params): Parameters<tools::fork::RestoreCheckpointParams>,
    ) -> Result<Json<tools::fork::RestoreCheckpointResponse>, McpError> {
        self.ensure_recalc_enabled("restore_checkpoint")
            .map_err(|e| to_mcp_error_for_tool("restore_checkpoint", e))?;
        self.run_tool_with_timeout(
            "restore_checkpoint",
            tools::fork::restore_checkpoint(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("restore_checkpoint", e))
    }

    #[tool(
        name = "delete_checkpoint",
        description = "Delete a checkpoint from a fork"
    )]
    pub async fn delete_checkpoint(
        &self,
        Parameters(params): Parameters<tools::fork::DeleteCheckpointParams>,
    ) -> Result<Json<tools::fork::DeleteCheckpointResponse>, McpError> {
        self.ensure_recalc_enabled("delete_checkpoint")
            .map_err(|e| to_mcp_error_for_tool("delete_checkpoint", e))?;
        self.run_tool_with_timeout(
            "delete_checkpoint",
            tools::fork::delete_checkpoint(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("delete_checkpoint", e))
    }

    #[tool(
        name = "list_staged_changes",
        description = "List previewed/staged changes for a fork"
    )]
    pub async fn list_staged_changes(
        &self,
        Parameters(params): Parameters<tools::fork::ListStagedChangesParams>,
    ) -> Result<Json<tools::fork::ListStagedChangesResponse>, McpError> {
        self.ensure_recalc_enabled("list_staged_changes")
            .map_err(|e| to_mcp_error_for_tool("list_staged_changes", e))?;
        self.run_tool_with_timeout(
            "list_staged_changes",
            tools::fork::list_staged_changes(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("list_staged_changes", e))
    }

    #[tool(
        name = "apply_staged_change",
        description = "Apply a staged change to a fork"
    )]
    pub async fn apply_staged_change(
        &self,
        Parameters(params): Parameters<tools::fork::ApplyStagedChangeParams>,
    ) -> Result<Json<tools::fork::ApplyStagedChangeResponse>, McpError> {
        self.ensure_recalc_enabled("apply_staged_change")
            .map_err(|e| to_mcp_error_for_tool("apply_staged_change", e))?;
        self.run_tool_with_timeout(
            "apply_staged_change",
            tools::fork::apply_staged_change(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("apply_staged_change", e))
    }

    #[tool(
        name = "discard_staged_change",
        description = "Discard a staged change without applying it"
    )]
    pub async fn discard_staged_change(
        &self,
        Parameters(params): Parameters<tools::fork::DiscardStagedChangeParams>,
    ) -> Result<Json<tools::fork::DiscardStagedChangeResponse>, McpError> {
        self.ensure_recalc_enabled("discard_staged_change")
            .map_err(|e| to_mcp_error_for_tool("discard_staged_change", e))?;
        self.run_tool_with_timeout(
            "discard_staged_change",
            tools::fork::discard_staged_change(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("discard_staged_change", e))
    }

    #[tool(
        name = "screenshot_sheet",
        description = "Capture a visual screenshot of a spreadsheet region as PNG. \
	Returns file URI. Max range: 100 rows x 30 columns. Default: A1:M40."
    )]
    pub async fn screenshot_sheet(
        &self,
        Parameters(params): Parameters<tools::fork::ScreenshotSheetParams>,
    ) -> Result<rmcp::model::CallToolResult, McpError> {
        use base64::Engine;
        use rmcp::model::Content;

        self.ensure_recalc_enabled("screenshot_sheet")
            .map_err(|e| to_mcp_error_for_tool("screenshot_sheet", e))?;

        let result = async {
            let response = self
                .run_tool_with_timeout(
                    "screenshot_sheet",
                    tools::fork::screenshot_sheet(self.state.clone(), params),
                )
                .await?;

            let mut content = Vec::new();

            let fs_path = response
                .output_path
                .strip_prefix("file://")
                .ok_or_else(|| anyhow!("unexpected screenshot output_path"))?;
            let bytes = tokio::fs::read(fs_path)
                .await
                .map_err(|e| anyhow!("failed to read screenshot: {}", e))?;

            if let Some(limit) = self.state.config().max_response_bytes() {
                let encoded_len = bytes.len().div_ceil(3) * 4;
                let meta = serde_json::to_vec(&response)
                    .map_err(|e| anyhow!("failed to serialize response: {}", e))?;
                let estimated = encoded_len + meta.len() + response.output_path.len();
                if estimated > limit {
                    return Err(
                        ResponseTooLargeError::new("screenshot_sheet", estimated, limit).into(),
                    );
                }
            }

            let data = base64::engine::general_purpose::STANDARD.encode(bytes);
            content.push(Content::image(data, "image/png"));

            // Always include a small text hint for clients that ignore structured_content.
            content.push(Content::text(response.output_path.clone()));

            let structured_content = to_pruned_value(&response)
                .map_err(|e| anyhow!("failed to serialize response: {}", e))?;

            Ok(rmcp::model::CallToolResult {
                content,
                structured_content: Some(structured_content),
                is_error: Some(false),
                meta: None,
            })
        }
        .await;

        result.map_err(|e| to_mcp_error_for_tool("screenshot_sheet", e))
    }
}

/// Per-family write tools subsumed by `mutate_batch`. Registered only in
/// compat mode (`SPREADSHEET_MCP_SLIM_SURFACE=false`).
#[cfg(feature = "recalc")]
#[tool_router(router = legacy_write_tool_router)]
impl SpreadsheetServer {
    #[tool(
        name = "transform_batch",
        description = "Range-oriented transforms for a fork (clear/fill/replace). Supports targets by range, region_id, or explicit cells. \
Mode: preview or apply (default apply)."
    )]
    pub async fn transform_batch(
        &self,
        Parameters(params): Parameters<tools::fork::TransformBatchParams>,
    ) -> Result<Json<tools::fork::TransformBatchResponse>, McpError> {
        self.ensure_recalc_enabled("transform_batch")
            .map_err(|e| to_mcp_error_for_tool("transform_batch", e))?;
        self.run_tool_with_timeout(
            "transform_batch",
            tools::fork::transform_batch(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("transform_batch", e))
    }
    #[tool(
        name = "style_batch",
        description = "Apply batch style edits to a fork. Supports targets by range, region_id, or explicit cells. \
Mode: preview or apply (default apply). Op mode: merge (default), set, or clear."
    )]
    pub async fn style_batch(
        &self,
        Parameters(params): Parameters<tools::fork::StyleBatchParamsInput>,
    ) -> Result<Json<tools::fork::StyleBatchResponse>, McpError> {
        self.ensure_recalc_enabled("style_batch")
            .map_err(|e| to_mcp_error_for_tool("style_batch", e))?;
        self.run_tool_with_timeout(
            "style_batch",
            tools::fork::style_batch(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("style_batch", e))
    }
    #[tool(
        name = "column_size_batch",
        description = "Set column widths or compute auto-widths in a fork. Targets column ranges like 'A:A' or 'A:C'. \
Mode: preview or apply (default apply). Auto computes and sets widths immediately (persisted). \
Note: autosize uses cached/formatted cell values; if a column is mostly formulas with no cached results, widths may be too narrow unless you recalculate first."
    )]
    pub async fn column_size_batch(
        &self,
        Parameters(params): Parameters<tools::fork::ColumnSizeBatchParamsInput>,
    ) -> Result<Json<tools::fork::ColumnSizeBatchResponse>, McpError> {
        self.ensure_recalc_enabled("column_size_batch")
            .map_err(|e| to_mcp_error_for_tool("column_size_batch", e))?;
        self.run_tool_with_timeout(
            "column_size_batch",
            tools::fork::column_size_batch(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("column_size_batch", e))
    }
    #[tool(
        name = "sheet_layout_batch",
        description = "Apply sheet layout/view/print settings in a fork (freeze panes, zoom, gridlines, margins, setup, print area, page breaks). Mode: preview or apply (default apply)."
    )]
    pub async fn sheet_layout_batch(
        &self,
        Parameters(params): Parameters<tools::sheet_layout::SheetLayoutBatchParams>,
    ) -> Result<Json<tools::sheet_layout::SheetLayoutBatchResponse>, McpError> {
        self.ensure_recalc_enabled("sheet_layout_batch")
            .map_err(|e| to_mcp_error_for_tool("sheet_layout_batch", e))?;
        self.run_tool_with_timeout(
            "sheet_layout_batch",
            tools::sheet_layout::sheet_layout_batch(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("sheet_layout_batch", e))
    }
    #[tool(
        name = "apply_formula_pattern",
        description = "Autofill-like formula pattern application over a target range in a fork. \
Provide base_formula at anchor_cell, then fill across target_range. \
Mode: preview or apply (default apply). relative_mode: excel (default), abs_cols, abs_rows. \
fill_direction: down, right, both (default both)."
    )]
    pub async fn apply_formula_pattern(
        &self,
        Parameters(params): Parameters<tools::fork::ApplyFormulaPatternParams>,
    ) -> Result<Json<tools::fork::ApplyFormulaPatternResponse>, McpError> {
        self.ensure_recalc_enabled("apply_formula_pattern")
            .map_err(|e| to_mcp_error_for_tool("apply_formula_pattern", e))?;
        self.run_tool_with_timeout(
            "apply_formula_pattern",
            tools::fork::apply_formula_pattern(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("apply_formula_pattern", e))
    }
    #[tool(
        name = "structure_batch",
        description = "Apply structural edits to a fork (rows/cols/sheets). \
Mode: preview or apply (default apply). Aliases: op for kind, add_sheet for create_sheet. \
Note: structural edits may not fully rewrite formulas/named ranges like Excel; run recalculate and review get_changeset after applying."
    )]
    pub async fn structure_batch(
        &self,
        Parameters(params): Parameters<tools::fork::StructureBatchParamsInput>,
    ) -> Result<Json<tools::fork::StructureBatchResponse>, McpError> {
        self.ensure_recalc_enabled("structure_batch")
            .map_err(|e| to_mcp_error_for_tool("structure_batch", e))?;
        self.run_tool_with_timeout(
            "structure_batch",
            tools::fork::structure_batch(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("structure_batch", e))
    }
    #[tool(
        name = "rules_batch",
        description = "Apply rule operations to a fork (DV v1: set_data_validation; CF v1: add/set/clear conditional formats). Mode: preview or apply (default apply)."
    )]
    pub async fn rules_batch(
        &self,
        Parameters(params): Parameters<tools::rules_batch::RulesBatchParams>,
    ) -> Result<Json<tools::rules_batch::RulesBatchResponse>, McpError> {
        self.ensure_recalc_enabled("rules_batch")
            .map_err(|e| to_mcp_error_for_tool("rules_batch", e))?;
        self.run_tool_with_timeout(
            "rules_batch",
            tools::rules_batch::rules_batch(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("rules_batch", e))
    }
    #[tool(
        name = "replace_in_formulas",
        description = "Find and replace text in formula bodies only (not cell values). \
Supports plain text and regex modes with optional case sensitivity. \
Scope to a range or default to the used range. \
Mode: preview or apply (default apply). \
Returns count of changed formulas and sample diffs."
    )]
    pub async fn replace_in_formulas(
        &self,
        Parameters(params): Parameters<tools::fork::ReplaceInFormulasParams>,
    ) -> Result<Json<tools::fork::ReplaceInFormulasResponse>, McpError> {
        self.ensure_recalc_enabled("replace_in_formulas")
            .map_err(|e| to_mcp_error_for_tool("replace_in_formulas", e))?;
        self.run_tool_with_timeout(
            "replace_in_formulas",
            tools::fork::replace_in_formulas(self.state.clone(), params),
        )
        .await
        .map(json)
        .map_err(|e| to_mcp_error_for_tool("replace_in_formulas", e))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SpreadsheetServer {
    fn get_info(&self) -> ServerInfo {
        let recalc_enabled = {
            #[cfg(feature = "recalc")]
            {
                self.state.config().recalc_enabled
            }
            #[cfg(not(feature = "recalc"))]
            {
                false
            }
        };

        let vba_enabled = self.state.config().vba_enabled;

        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(build_instructions(recalc_enabled, vba_enabled)),
            ..ServerInfo::default()
        }
    }
}

fn to_mcp_error_for_tool(tool: &str, error: anyhow::Error) -> McpError {
    if error.is::<ToolDisabledError>() || error.is::<ResponseTooLargeError>() {
        return McpError::invalid_request(error.to_string(), None);
    }

    if let Some(inv) = error.downcast_ref::<InvalidParamsError>() {
        let example = tool_minimal_example(tool);
        let variants = tool_variants(tool, inv.message())
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let msg = format_invalid_params_message(
            tool,
            inv.message(),
            inv.path(),
            if variants.is_empty() {
                None
            } else {
                Some(&variants)
            },
            example,
        );
        return McpError::invalid_params(msg, None);
    }

    if let Some(serde_err) = error.downcast_ref::<serde_json::Error>() {
        let problem = serde_err.to_string();
        let path = infer_path_for_tool(tool, &problem);

        let mut variants = extract_expected_variants(&problem);
        if variants.is_empty()
            && let Some(extra) = tool_variants(tool, &problem)
        {
            variants = extra.into_iter().map(|s| s.to_string()).collect();
        }

        let example = tool_minimal_example(tool);
        let msg = format_invalid_params_message(
            tool,
            &problem,
            path.as_deref(),
            if variants.is_empty() {
                None
            } else {
                Some(&variants)
            },
            example,
        );
        return McpError::invalid_params(msg, None);
    }

    // Heuristic fallbacks for common user-caused shape/enum mistakes that may not
    // be typed as serde_json::Error (e.g., anyhow::bail! paths).
    let problem = error.to_string();
    if looks_like_invalid_params(&problem) {
        let path = infer_path_for_tool(tool, &problem);
        let variants = tool_variants(tool, &problem)
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let example = tool_minimal_example(tool);
        let msg = format_invalid_params_message(
            tool,
            &problem,
            path.as_deref(),
            if variants.is_empty() {
                None
            } else {
                Some(&variants)
            },
            example,
        );
        return McpError::invalid_params(msg, None);
    }

    McpError::internal_error(problem, None)
}

fn format_invalid_params_message(
    tool: &str,
    problem: &str,
    path: Option<&str>,
    variants: Option<&[String]>,
    example: Option<&'static str>,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("Invalid params for tool '{tool}': {problem}"));

    if let Some(path) = path {
        out.push_str(&format!("\npath: {path}"));
    }

    if let Some(variants) = variants
        && !variants.is_empty()
    {
        out.push_str("\nvalid variants: ");
        out.push_str(&variants.join(", "));
    }

    if let Some(example) = example {
        out.push_str("\nexample: ");
        out.push_str(example);
    }

    out
}

fn tool_minimal_example(tool: &str) -> Option<&'static str> {
    match tool {
        "mutate_batch" => Some(
            r#"{"fork_id":"<fork_id>","mode":"apply","ops":[{"kind":"fill_range","sheet_name":"Sheet1","target":{"kind":"range","range":"A1:A3"},"value":"0"},{"kind":"insert_rows","sheet_name":"Sheet1","at_row":2,"count":1}]}"#,
        ),
        "structure_batch" => Some(
            r#"{"fork_id":"<fork_id>","ops":[{"kind":"insert_rows","sheet_name":"Sheet1","at_row":2,"count":1}],"mode":"apply"}"#,
        ),
        "style_batch" => Some(
            r#"{"fork_id":"<fork_id>","ops":[{"sheet_name":"Sheet1","target":{"kind":"range","range":"A1:A1"},"patch":{"fill":{"kind":"pattern","pattern_type":"solid","foreground_color":"FFFF0000"}},"op_mode":"merge"}],"mode":"apply"}"#,
        ),
        "edit_batch" => Some(
            r#"{"fork_id":"<fork_id>","sheet_name":"Sheet1","edits":["A1=100","B2==SUM(A1:A2)"]}"#,
        ),
        "sheet_layout_batch" => Some(
            r#"{"fork_id":"<fork_id>","ops":[{"kind":"freeze_panes","sheet_name":"Dashboard","freeze_rows":1,"freeze_cols":1}],"mode":"apply"}"#,
        ),
        "rules_batch" => Some(
            r#"{"fork_id":"<fork_id>","ops":[{"kind":"set_data_validation","sheet_name":"Inputs","target_range":"B3:B100","validation":{"kind":"list","formula1":"=Lists!$A$1:$A$10","allow_blank":false}}],"mode":"apply"}"#,
        ),
        _ => None,
    }
}

fn infer_path_for_tool(tool: &str, problem: &str) -> Option<String> {
    let p = problem.to_ascii_lowercase();

    match tool {
        "structure_batch" => {
            if p.contains("structure op") && (p.contains("kind") || p.contains("op")) {
                return Some("ops[0].kind".to_string());
            }
            if p.contains("missing field `kind`") || p.contains("missing field kind") {
                return Some("ops[0].kind".to_string());
            }
            None
        }
        "style_batch" => {
            if p.contains("fillpatch") || p.contains("fillpatchinput") {
                return Some("ops[0].patch.fill.kind".to_string());
            }
            if p.contains("styletarget") && p.contains("kind") {
                return Some("ops[0].target.kind".to_string());
            }
            None
        }
        "sheet_layout_batch" => {
            if p.contains("missing field `kind`") || p.contains("missing field kind") {
                return Some("ops[0].kind".to_string());
            }
            if p.contains("sheetlayoutop") && p.contains("kind") {
                return Some("ops[0].kind".to_string());
            }
            if p.contains("unknown variant") && p.contains("apply") && p.contains("preview") {
                return Some("mode".to_string());
            }
            if p.contains("mode") && p.contains("invalid") {
                return Some("mode".to_string());
            }
            None
        }
        "rules_batch" => {
            if p.contains("missing field `kind`") || p.contains("missing field kind") {
                return Some("ops[0].kind".to_string());
            }
            if p.contains("rulesop") && p.contains("kind") {
                return Some("ops[0].kind".to_string());
            }
            if p.contains("datavalidationkind") {
                return Some("ops[0].validation.kind".to_string());
            }
            if p.contains("conditionalformat") && p.contains("operator") {
                return Some("ops[0].rule.operator".to_string());
            }
            if p.contains("conditionalformatrulespec") && p.contains("kind") {
                return Some("ops[0].rule.kind".to_string());
            }
            if p.contains("unknown variant") && p.contains("apply") && p.contains("preview") {
                return Some("mode".to_string());
            }
            if p.contains("mode") && p.contains("invalid") {
                return Some("mode".to_string());
            }
            None
        }
        _ => None,
    }
}

fn tool_variants(tool: &str, problem: &str) -> Option<Vec<&'static str>> {
    let p = problem.to_ascii_lowercase();

    match tool {
        "mutate_batch" => {
            if p.contains("kind") {
                return Some(agent_spreadsheet::tools::mutate_batch::all_mutate_op_kinds());
            }
            None
        }
        "structure_batch" => {
            if p.contains("structure op")
                || p.contains("structureop")
                || (p.contains("unknown variant") && p.contains("kind"))
            {
                return Some(vec![
                    "insert_rows",
                    "delete_rows",
                    "insert_cols",
                    "delete_cols",
                    "rename_sheet",
                    "create_sheet",
                    "delete_sheet",
                    "copy_range",
                    "move_range",
                ]);
            }
            None
        }
        "style_batch" => {
            if p.contains("fill") || p.contains("fillpatch") || p.contains("fillpatchinput") {
                return Some(vec!["pattern", "gradient"]);
            }
            if p.contains("op_mode") || p.contains("op mode") {
                return Some(vec!["merge", "set", "clear"]);
            }
            None
        }
        "sheet_layout_batch" => {
            if p.contains("sheetlayoutop")
                || p.contains("sheet layout op")
                || (p.contains("unknown variant") && p.contains("kind"))
                || p.contains("missing field `kind`")
                || p.contains("missing field kind")
            {
                return Some(vec![
                    "freeze_panes",
                    "set_zoom",
                    "set_gridlines",
                    "set_page_margins",
                    "set_page_setup",
                    "set_print_area",
                    "set_page_breaks",
                ]);
            }
            None
        }
        "rules_batch" => {
            if p.contains("rulesop")
                || p.contains("rules op")
                || (p.contains("unknown variant") && p.contains("kind"))
                || p.contains("missing field `kind`")
                || p.contains("missing field kind")
            {
                return Some(vec![
                    "set_data_validation",
                    "add_conditional_format",
                    "set_conditional_format",
                    "clear_conditional_formats",
                ]);
            }

            if p.contains("datavalidationkind") {
                return Some(vec!["list", "whole", "decimal", "date", "custom"]);
            }
            if p.contains("conditionalformatrulespec") {
                return Some(vec!["cell_is", "expression"]);
            }
            if p.contains("conditionalformatoperator") {
                return Some(vec![
                    "less_than",
                    "less_than_or_equal",
                    "greater_than",
                    "greater_than_or_equal",
                    "equal",
                    "not_equal",
                    "between",
                    "not_between",
                ]);
            }
            None
        }
        _ => None,
    }
}

fn looks_like_invalid_params(problem: &str) -> bool {
    let p = problem.to_ascii_lowercase();

    // serde-driven shape/enum failures
    if p.contains("missing field")
        || p.contains("unknown field")
        || p.contains("unknown variant")
        || p.contains("did not match any variant")
        || p.contains("must be an object")
    {
        return true;
    }

    // common hand-rolled validation errors
    if p.contains("invalid shorthand edit") {
        return true;
    }

    if p.contains("invalid mode") {
        return true;
    }

    false
}

fn extract_expected_variants(problem: &str) -> Vec<String> {
    static EXPECTED_TAIL_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"expected(?: one of)? (?P<tail>.*)$").expect("regex"));
    static BACKTICK_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"`([^`]+)`").expect("regex"));

    let Some(caps) = EXPECTED_TAIL_RE.captures(problem) else {
        return Vec::new();
    };
    let tail = caps.name("tail").map(|m| m.as_str()).unwrap_or("");
    BACKTICK_RE
        .captures_iter(tail)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .collect()
}

#[cfg(all(test, feature = "recalc"))]
mod typed_errors_tests {
    use super::to_mcp_error_for_tool;
    use crate::tools;
    use rmcp::model::ErrorCode;
    use serde_json::json;

    #[test]
    fn structure_batch_missing_kind_or_op_is_invalid_params_with_example_and_variants() {
        let bad = json!({
            "fork_id": "f1",
            "ops": [
                { "sheet_name": "Sheet1", "at_row": 2, "count": 1 }
            ]
        });

        let err =
            serde_json::from_value::<tools::fork::StructureBatchParamsInput>(bad).unwrap_err();
        let mcp = to_mcp_error_for_tool("structure_batch", err.into());

        assert_eq!(mcp.code, ErrorCode::INVALID_PARAMS);
        assert!(mcp.message.to_ascii_lowercase().contains("example:"));
        assert!(mcp.message.contains("insert_rows"));
        assert!(mcp.message.to_ascii_lowercase().contains("valid variants"));
    }

    #[test]
    fn style_batch_fill_missing_kind_is_invalid_params_with_example_and_variants() {
        let bad = json!({
            "fork_id": "f1",
            "ops": [
                {
                    "sheet_name": "Sheet1",
                    "target": { "kind": "range", "range": "A1:A1" },
                    "patch": {
                        "fill": { "pattern_type": "solid", "foreground_color": "FFFF0000" }
                    }
                }
            ]
        });

        let err = serde_json::from_value::<tools::fork::StyleBatchParamsInput>(bad).unwrap_err();
        let mcp = to_mcp_error_for_tool("style_batch", err.into());

        assert_eq!(mcp.code, ErrorCode::INVALID_PARAMS);
        assert!(mcp.message.to_ascii_lowercase().contains("example:"));
        assert!(mcp.message.contains("pattern"));
        assert!(mcp.message.to_ascii_lowercase().contains("valid variants"));
    }

    #[test]
    fn edit_batch_shorthand_missing_equals_is_invalid_params_with_example() {
        let params = tools::write_normalize::EditBatchParamsInput {
            fork_id: "f1".to_string(),
            sheet_name: "Sheet1".to_string(),
            edits: vec![tools::write_normalize::CellEditInput::Shorthand(
                "A1".to_string(),
            )],

            mode: None,
            formula_parse_policy: None,
        };

        let err = tools::write_normalize::normalize_edit_batch(params).unwrap_err();
        let mcp = to_mcp_error_for_tool("edit_batch", err);

        assert_eq!(mcp.code, ErrorCode::INVALID_PARAMS);
        assert!(mcp.message.to_ascii_lowercase().contains("example:"));
        assert!(mcp.message.contains("A1=100"));
    }

    #[test]
    fn sheet_layout_batch_missing_kind_is_invalid_params_with_example_and_variants() {
        let bad = json!({
            "fork_id": "f1",
            "ops": [
                { "sheet_name": "Dashboard", "freeze_rows": 1, "freeze_cols": 1 }
            ],
            "mode": "apply"
        });

        let err =
            serde_json::from_value::<tools::sheet_layout::SheetLayoutBatchParams>(bad).unwrap_err();
        let mcp = to_mcp_error_for_tool("sheet_layout_batch", err.into());

        assert_eq!(mcp.code, ErrorCode::INVALID_PARAMS);
        assert!(mcp.message.to_ascii_lowercase().contains("example:"));
        assert!(mcp.message.to_ascii_lowercase().contains("valid variants"));
        assert!(mcp.message.contains("freeze_panes"));
    }

    #[test]
    fn rules_batch_missing_kind_is_invalid_params_with_example_and_variants() {
        let bad = json!({
            "fork_id": "f1",
            "ops": [
                {
                    "sheet_name": "Inputs",
                    "target_range": "B3:B10",
                    "validation": { "kind": "list", "formula1": "=Lists!$A$1:$A$10" }
                }
            ],
            "mode": "apply"
        });

        let err = serde_json::from_value::<tools::rules_batch::RulesBatchParams>(bad).unwrap_err();
        let mcp = to_mcp_error_for_tool("rules_batch", err.into());

        assert_eq!(mcp.code, ErrorCode::INVALID_PARAMS);
        assert!(mcp.message.to_ascii_lowercase().contains("example:"));
        assert!(mcp.message.to_ascii_lowercase().contains("valid variants"));
        assert!(mcp.message.contains("set_data_validation"));
    }

    #[test]
    fn rules_batch_invalid_mode_is_invalid_params_with_example_and_path() {
        let bad = json!({
            "fork_id": "f1",
            "ops": [
                {
                    "kind": "set_data_validation",
                    "sheet_name": "Inputs",
                    "target_range": "B3:B10",
                    "validation": { "kind": "list", "formula1": "=Lists!$A$1:$A$10" }
                }
            ],
            "mode": "maybe"
        });

        let err = serde_json::from_value::<tools::rules_batch::RulesBatchParams>(bad).unwrap_err();
        let mcp = to_mcp_error_for_tool("rules_batch", err.into());

        assert_eq!(mcp.code, ErrorCode::INVALID_PARAMS);
        assert!(mcp.message.to_ascii_lowercase().contains("example:"));
        assert!(mcp.message.to_ascii_lowercase().contains("path: mode"));
    }
}

#[derive(Debug, Error)]
#[error("tool '{tool_name}' is disabled by server configuration")]
struct ToolDisabledError {
    tool_name: String,
}

impl ToolDisabledError {
    fn new(tool_name: &str) -> Self {
        Self {
            tool_name: tool_name.to_ascii_lowercase(),
        }
    }
}

#[derive(Debug, Error)]
#[error(
    "tool '{tool_name}' response too large ({size} bytes > {limit} bytes); reduce request size or page results"
)]
struct ResponseTooLargeError {
    tool_name: String,
    size: usize,
    limit: usize,
}

impl ResponseTooLargeError {
    fn new(tool_name: &str, size: usize, limit: usize) -> Self {
        Self {
            tool_name: tool_name.to_ascii_lowercase(),
            size,
            limit,
        }
    }
}

#[derive(Debug, Error)]
#[error("VBA tools are disabled (set SPREADSHEET_MCP_VBA_ENABLED=true)")]
struct VbaDisabledError;

#[cfg(feature = "recalc")]
#[derive(Debug, Error)]
#[error("recalc/write tools are disabled (set SPREADSHEET_MCP_RECALC_ENABLED=true)")]
struct RecalcDisabledError;
