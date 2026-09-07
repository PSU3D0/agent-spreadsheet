# Surface Capability Matrix (CLI / MCP / WASM / SDK)

Status: active migration baseline (Wave 4 canonical MCP adapter)
Owner: Tranche 35 (tickets/35-js-surface-migration)

This matrix is the planning baseline for cross-surface migration.

## Resident session amendment (implementation planned)

`resident-session-runtime.md` extends explicit session bindings across native CLI/MCP and portable WASM/SDK/just-bash. File-bound CLI/just-bash remains ephemeral. Session mutation/evaluation/history semantics belong to the shared Rust runtime; host adapters own storage and process/VFS lifecycle. Native session commands may auto-start a private host. Portable memory-only versus host-backed persistence is explicit; stage/checkpoint catalogs have separate generations from workbook CAS revisions. Atomic implementations may use bounded reversible changes instead of full-state copies when the shared recovery guarantees hold.

These are authorized design boundaries, **not current capability claims**. Existing catalog rows and live discovery remain authoritative until their resident implementations and cross-surface gates land. The old `MCP_ONLY` orchestration classification does not prohibit implementing the same session semantics through other backed hosts; adapter-specific discovery must still reflect actual support.

## Legend

- **Classification**
  - `ALL` = intended shared capability (CLI + MCP + WASM via shared core)
  - `CLI_ONLY` = host/operator concern (no MCP/WASM parity required)
  - `MCP_ONLY` = agent/session orchestration concern
  - `SHARED_PARTIAL` = shared semantics, but currently only implemented on subset
- **WASM target**
  - `mvp` = planned for initial WASM surface
  - `later` = planned after MVP
  - `n/a` = intentionally not a WASM concern

Boundary contract: `docs/architecture/surface-boundary-rules.md`

---

## A) CLI command catalog

| CLI command/subcommand | MCP equivalent tool(s) | Classification | Core projection target | WASM target | Notes | Implementation module path | Parity test owner |
|---|---|---:|---|---:|---|---|---|
| `read sheets` | `list_sheets` | ALL | `core.read.list_sheets` | mvp | Shared read primitive | `crates/agent-spreadsheet/src/cli/commands/read.rs::list_sheets` | `crates/agent-spreadsheet/tests/core_runtime_parity.rs` |
| `read overview` | `sheet_overview` | ALL | `core.read.sheet_overview` | mvp | Shared read primitive | `crates/agent-spreadsheet/src/cli/commands/read.rs::sheet_overview` | `crates/agent-spreadsheet/tests/sheet_overview_truncation.rs` |
| `read values` | `range_values` | ALL | `core.read.range_values` | mvp | Shared read primitive | `crates/agent-spreadsheet/src/cli/commands/read.rs::range_values` | `crates/agent-spreadsheet/tests/core_runtime_parity.rs` |
| `read export --format json/csv` | `range_values` | ALL | `core.read.range_values` + formatter | mvp | CSV serialization shared; CLI handles output path/stdout | `crates/agent-spreadsheet/src/cli/commands/read.rs::range_export` | `crates/agent-spreadsheet/tests/cli_integration.rs` |
| `read export --format grid` | `grid_export` | ALL | `core.read.grid_export` | mvp | Rich payload export | `crates/agent-spreadsheet/src/cli/commands/read.rs::range_export` | `crates/agent-spreadsheet/tests/unit_grid_roundtrip.rs` |
| `write import --from-grid` | `grid_import` | ALL | `core.write.grid_import` | mvp | Shared grid import semantics | `crates/agent-spreadsheet/src/cli/commands/write.rs::range_import` | `crates/agent-spreadsheet/tests/unit_grid_roundtrip.rs` |
| `write import --from-csv` | _(none today)_ | SHARED_PARTIAL | `core.write.csv_import` | mvp | CLI has path; MCP may add later | `crates/agent-spreadsheet/src/cli/commands/write.rs::range_import` | `crates/agent-spreadsheet/tests/cli_integration.rs` |
| `read cells` | `inspect_cells` | ALL | `core.read.inspect_cells` | mvp | Strict detail-view: up to 25 cells with full metadata; returns budget object | `crates/agent-spreadsheet/src/cli/commands/read.rs::inspect_cells` | `crates/agent-spreadsheet/tests/read_guardrails.rs` |
| `read page` | `sheet_page` | ALL | `core.read.sheet_page` | mvp | Shared pagination contract | `crates/agent-spreadsheet/src/cli/commands/read.rs::sheet_page` | `crates/agent-spreadsheet/tests/core_runtime_parity.rs` |
| `read table` | `read_table` | ALL | `core.read.read_table` | mvp | Shared table read primitive | `crates/agent-spreadsheet/src/cli/commands/read.rs::read_table` | `crates/agent-spreadsheet/tests/read_table_polish.rs` |
| `analyze find-value` | `find_value` | ALL | `core.analysis.find_value` | mvp | Shared analysis primitive | `crates/agent-spreadsheet/src/cli/commands/read.rs::find_value` | `crates/agent-spreadsheet/tests/core_runtime_parity.rs` |
| `read names` | `named_ranges` | ALL | `core.read.named_ranges` | mvp | Shared read primitive | `crates/agent-spreadsheet/src/cli/commands/read.rs::named_ranges` | `crates/agent-spreadsheet/tests/core_runtime_parity.rs` |
| `write name define` | `define_name` | ALL | `core.write.define_name` | mvp | Named range CRUD (create) | `crates/agent-spreadsheet/src/cli/commands/write.rs::define_name` | `crates/agent-spreadsheet/tests/cli_integration.rs` |
| `write name update` | `update_name` | ALL | `core.write.update_name` | mvp | Named range CRUD (update) | `crates/agent-spreadsheet/src/cli/commands/write.rs::update_name` | `crates/agent-spreadsheet/tests/cli_integration.rs` |
| `write name delete` | `delete_name` | ALL | `core.write.delete_name` | mvp | Named range CRUD (delete) | `crates/agent-spreadsheet/src/cli/commands/write.rs::delete_name` | `crates/agent-spreadsheet/tests/cli_integration.rs` |
| `analyze find-formula` | `find_formula` | ALL | `core.analysis.find_formula` | mvp | Shared analysis primitive | `crates/agent-spreadsheet/src/cli/commands/read.rs::find_formula` | `crates/agent-spreadsheet/tests/core_runtime_parity.rs` |
| `analyze scan-volatiles` | `scan_volatiles` | ALL | `core.analysis.scan_volatiles` | mvp | Shared analysis primitive | `crates/agent-spreadsheet/src/cli/commands/read.rs::scan_volatiles` | `crates/agent-spreadsheet/tests/core_runtime_parity.rs` |
| `analyze sheet-statistics` | `sheet_statistics` | ALL | `core.analysis.sheet_statistics` | mvp | Shared analysis primitive | `crates/agent-spreadsheet/src/cli/commands/read.rs::sheet_statistics` | `crates/agent-spreadsheet/tests/core_runtime_parity.rs` |
| `analyze formula-map` | `sheet_formula_map` | ALL | `core.analysis.sheet_formula_map` | mvp | Shared analysis primitive | `crates/agent-spreadsheet/src/cli/commands/read.rs::formula_map` | `crates/agent-spreadsheet/tests/heuristic_scenarios.rs` |
| `analyze formula-trace` | `formula_trace` | ALL | `core.analysis.formula_trace` | later | Shared but heavier graph concerns | `crates/agent-spreadsheet/src/cli/commands/read.rs::formula_trace` | `crates/agent-spreadsheet/tests/core_runtime_parity.rs` |
| `read workbook` | `describe_workbook` | ALL | `core.read.describe_workbook` | mvp | Contract naming differs by surface | `crates/agent-spreadsheet/src/cli/commands/read.rs::describe` | `crates/agent-spreadsheet/tests/core_runtime_parity.rs` |
| `analyze table-profile` | `table_profile` | ALL | `core.analysis.table_profile` | mvp | Shared profiling primitive | `crates/agent-spreadsheet/src/cli/commands/read.rs::table_profile` | `crates/agent-spreadsheet/tests/read_table_polish.rs` |
| `read layout` | `layout_page` | ALL | `core.read.layout_page` | mvp | Shared layout primitive | `crates/agent-spreadsheet/src/cli/commands/read.rs::layout_page` | `crates/agent-spreadsheet/tests/unit_layout_page.rs` |
| `workbook create` | _(none today)_ | SHARED_PARTIAL | `core.write.create_workbook_bytes` (planned) | later | CLI path-based today | `crates/agent-spreadsheet/src/cli/commands/write.rs::create_workbook` | `crates/agent-spreadsheet/tests/cli_integration.rs` |
| `workbook copy` | _(none today)_ | CLI_ONLY | `adapter-cli.copy_path` | n/a | Stateless file orchestration | `crates/agent-spreadsheet/src/cli/commands/write.rs::copy` | `crates/agent-spreadsheet/tests/cli_integration.rs` |
| `write cells` | `edit_batch` | ALL | `core.write.edit_batch` | mvp | CLI shorthand parsing is adapter concern | `crates/agent-spreadsheet/src/cli/commands/write.rs::edit` | `crates/agent-spreadsheet/tests/unit_edit_batch.rs` |
| `write batch transform` | `transform_batch` | ALL | `core.write.transform_batch` | mvp | Shared write primitive | `crates/agent-spreadsheet/src/cli/commands/write.rs::transform_batch` | `crates/agent-spreadsheet/tests/core_runtime_parity.rs` |
| `write batch style` | `style_batch` | ALL | `core.write.style_batch` | mvp | Shared write primitive | `crates/agent-spreadsheet/src/cli/commands/write.rs::style_batch` | `crates/agent-spreadsheet/tests/core_runtime_parity.rs` |
| `write batch formula-pattern` | `apply_formula_pattern` | ALL | `core.write.apply_formula_pattern` | later | Shared write primitive | `crates/agent-spreadsheet/src/cli/commands/write.rs::apply_formula_pattern` | `crates/agent-spreadsheet/tests/unit_formula_pattern.rs` |
| `write batch structure` | `structure_batch` | ALL | `core.write.structure_batch` | later | Shared write primitive | `crates/agent-spreadsheet/src/cli/commands/write.rs::structure_batch` | `crates/agent-spreadsheet/tests/core_runtime_parity.rs` |
| `write batch column-size` | `column_size_batch` | ALL | `core.write.column_size_batch` | later | Shared write primitive | `crates/agent-spreadsheet/src/cli/commands/write.rs::column_size_batch` | `crates/agent-spreadsheet/tests/core_runtime_parity.rs` |
| `write batch sheet-layout` | `sheet_layout_batch` | ALL | `core.write.sheet_layout_batch` | later | Shared write primitive | `crates/agent-spreadsheet/src/cli/commands/write.rs::sheet_layout_batch` | `crates/agent-spreadsheet/tests/core_runtime_parity.rs` |
| `write batch rules` | `rules_batch` | ALL | `core.write.rules_batch` | later | Shared write primitive | `crates/agent-spreadsheet/src/cli/commands/write.rs::rules_batch` | `crates/agent-spreadsheet/tests/core_runtime_parity.rs` |
| `write formulas replace` | `replace_in_formulas` | ALL | `core.write.replace_in_formulas` | later | Formula-only find/replace with dry-run | `crates/agent-spreadsheet/src/cli/commands/write.rs::replace_in_formulas` | `crates/agent-spreadsheet/tests/unit_replace_in_formulas.rs` |
| `sheetport manifest candidates` | `get_manifest_stub` | ALL | `canonical_optional.sheetport_manifest` | later | Legacy aliases share canonical candidate semantics | `crates/agent-spreadsheet/src/cli/commands/read.rs::sheetport_manifest_candidates` | `crates/agent-spreadsheet/tests/cli_integration.rs` |
| `sheetport manifest schema` | `sheetport_manifest` | ALL | `canonical_optional.sheetport_schema` | later | CLI only owns printing; schema semantics are core | `crates/agent-spreadsheet/src/cli/commands/read.rs::sheetport_manifest_schema` | `crates/agent-spreadsheet/tests/cli_integration.rs` |
| `sheetport manifest validate` | `sheetport_manifest` | ALL | `canonical_optional.validate_manifest_content` | later | CLI owns file input; core validates portable content | `crates/agent-spreadsheet/src/cli/commands/read.rs::sheetport_manifest_validate` | `crates/agent-spreadsheet/tests/cli_integration.rs` |
| `sheetport manifest normalize` | `sheetport_manifest` | ALL | `canonical_optional.normalize_manifest_content` | later | CLI owns file output; core normalizes portable content | `crates/agent-spreadsheet/src/cli/commands/read.rs::sheetport_manifest_normalize` | `crates/agent-spreadsheet/tests/cli_integration.rs` |
| `sheetport bind-check` | `sheetport_manifest` | ALL | `canonical_optional.bind_check_manifest_content` | later | Legacy CLI response remains compatible | `crates/agent-spreadsheet/src/cli/commands/read.rs::sheetport_bind_check` | `crates/agent-spreadsheet/tests/cli_integration.rs` |
| `sheetport run` | `execute_manifest` | ALL | `canonical_optional.execute_sheetport` | later | Canonical layer adds typed values and explicit coverage/errors | `crates/agent-spreadsheet/src/cli/commands/read.rs::sheetport_run` | `crates/agent-spreadsheet/tests/cli_integration.rs` |
| `workbook recalculate` | `recalculate` | ALL | `core.recalc.recalculate` | mvp | WASM uses the Formualizer in-memory evaluator; LibreOffice remains native-only | `crates/agent-spreadsheet/src/cli/commands/recalc.rs::recalculate` | `crates/agent-spreadsheet/tests/cli_integration.rs` |
| `verify proof` | `verify_workbook` | ALL | `core.verify.compare_workbooks` | mvp | WASM binds two persistent session ids and evaluates both entirely in memory | `crates/agent-spreadsheet/src/cli/commands/verify.rs::verify` | `crates/agent-spreadsheet/tests/cli_integration.rs` |
| `write append` | `write` (`append_rows`) | ALL | `core.write_planner.apply_append_rows` | mvp | Core owns region/footer append policy and canonical enums; CLI parses file/row inputs and projects plans | `crates/agent-spreadsheet/src/core/write_planner.rs` | `crates/agent-spreadsheet/tests/canonical_write.rs` |
| `write clone-template-row` | `write` (`clone_row`) | ALL | `core.write_planner.apply_clone_row` | mvp | Core owns row clone semantics and canonical patch/merge policies; CLI remains an adapter | `crates/agent-spreadsheet/src/core/write_planner.rs` | `crates/agent-spreadsheet/tests/canonical_write.rs` |
| `write clone-row-band` | `write` (`clone_row_band`) | ALL | `core.write_planner.apply_clone_row_band` | mvp | Core owns contiguous band insertion/copy semantics and canonical enums; CLI remains an adapter | `crates/agent-spreadsheet/src/core/write_planner.rs` | `crates/agent-spreadsheet/tests/canonical_write.rs` |
| `verify diff` | `get_changeset` (partial overlap) | SHARED_PARTIAL | `core.diff.diff_workbooks` | later | CLI is file-vs-file; MCP is fork-oriented; CLI now projects grouped summary buckets and can suppress `recalc_result` noise | `crates/agent-spreadsheet/src/cli/commands/diff.rs::diff` | `crates/agent-spreadsheet/tests/diff_engine.rs` |
| `analyze ref-impact` | _(none today)_ | CLI_ONLY | `core.analysis.structure_impact` | n/a | Read-only structural impact preflight; uses same engine as `structure-batch --dry-run --impact-report` | `crates/agent-spreadsheet/src/cli/commands/write.rs::check_ref_impact` | `crates/agent-spreadsheet/tests/cli_integration.rs` |
| `operations` | _(registry discovery)_ | ALL | `operations.operation_registry` | mvp | Lists only operations executable by the stateless CLI adapter; durable orchestration is hidden | `crates/agent-spreadsheet/src/operations.rs::operation_registry` | `crates/agent-spreadsheet/tests/canonical_operations.rs` |
| `op` | _(canonical dispatcher)_ | ALL | `operations.execute_operation` | mvp | Descriptor adapter metadata drives none/read/mutable/two-resource/durable binding and export policy; verification uses `--baseline` plus `--bind` | `crates/agent-spreadsheet/src/cli/mod.rs::run_machine_operation` | `crates/agent-spreadsheet/tests/canonical_operations.rs` |
| `registry --all` | _(complete registry projection)_ | ALL | `operations.registry_projection` | mvp | Emits all 31 descriptors and schemas without runtime availability filtering for generators | `crates/agent-spreadsheet/src/operations.rs::registry_projection` | `crates/agent-spreadsheet/tests/canonical_operations.rs` |
| `schema` | _(canonical registry)_ | ALL | `operations.operation_schema` | mvp | Canonical operation schemas are generated collision-free from registry descriptors; legacy batch/session targets remain adapter projections | `crates/agent-spreadsheet/src/cli/mod.rs::run` | `crates/agent-spreadsheet/tests/canonical_operations.rs` |
| `example` | _(canonical registry)_ | ALL | `operations.operation_registry` input schemas | mvp | Canonical examples are generated collision-free from descriptor input schemas; legacy batch/session examples remain adapter projections | `crates/agent-spreadsheet/src/cli/mod.rs::canonical_operation_example` | `crates/agent-spreadsheet/tests/canonical_operations.rs` |
| `session` | _(none today)_ | CLI_ONLY | `core.session.*` | n/a | Event-sourced session management (start, log, branches, switch, checkout, undo, redo, fork, op, apply, materialize) | `crates/agent-spreadsheet/src/cli/commands/session.rs` | `crates/agent-spreadsheet/tests/cli_integration.rs` |

---

## B) MCP tool catalog

| MCP tool | CLI equivalent | Classification | Core projection target | WASM target | Notes | Implementation module path | Parity test owner |
|---|---|---:|---|---:|---|---|---|
| `read_cells` | `read cells`/`read page` | ALL | `operations.read_cells` | mvp | Canonical correlated exact/projected read | `crates/agent-spreadsheet-mcp/src/canonical_router.rs` | `crates/agent-spreadsheet-mcp/tests/canonical_projection.rs` |
| `read_layout` | `read layout` | ALL | `operations.read_layout` | mvp | Canonical bounded lossy layout read | `crates/agent-spreadsheet-mcp/src/canonical_router.rs` | `crates/agent-spreadsheet-mcp/tests/canonical_projection.rs` |
| `export_grid` | `read export --format grid` | ALL | `operations.export_grid` | mvp | Canonical coordinate-preserving grid export | `crates/agent-spreadsheet-mcp/src/canonical_router.rs` | `crates/agent-spreadsheet-mcp/tests/canonical_projection.rs` |
| `analyze_styles` | _(none direct)_ | ALL | `operations.analyze_styles` | later | Canonical workbook/sheet scoped style analysis | `crates/agent-spreadsheet-mcp/src/canonical_router.rs` | `crates/agent-spreadsheet-mcp/tests/canonical_projection.rs` |
| `search_values` | `analyze find-value` | ALL | `operations.search_values` | mvp | Canonical scoped value/label search | `crates/agent-spreadsheet-mcp/src/canonical_router.rs` | `crates/agent-spreadsheet-mcp/tests/canonical_projection.rs` |
| `search_formulas` | `analyze find-formula`/`scan-volatiles` | ALL | `operations.search_formulas` | mvp | Canonical formula search and classification | `crates/agent-spreadsheet-mcp/src/canonical_router.rs` | `crates/agent-spreadsheet-mcp/tests/canonical_projection.rs` |
| `formula_map` | `analyze formula-map` | ALL | `operations.formula_map` | mvp | Canonical formula topology map | `crates/agent-spreadsheet-mcp/src/canonical_router.rs` | `crates/agent-spreadsheet-mcp/tests/canonical_projection.rs` |
| `profile_table` | `analyze table-profile` | ALL | `operations.profile_table` | mvp | Canonical table profiling | `crates/agent-spreadsheet-mcp/src/canonical_router.rs` | `crates/agent-spreadsheet-mcp/tests/canonical_projection.rs` |
| `sheetport_manifest` | `sheetport manifest ...` | ALL | `operations.sheetport_manifest` | later | Capability-gated canonical SheetPort manifest actions | `crates/agent-spreadsheet-mcp/src/canonical_router.rs` | `crates/agent-spreadsheet-mcp/tests/canonical_projection.rs` |
| `execute_sheetport` | `sheetport run` | ALL | `operations.execute_sheetport` | later | Capability-gated canonical SheetPort execution | `crates/agent-spreadsheet-mcp/src/canonical_router.rs` | `crates/agent-spreadsheet-mcp/tests/canonical_projection.rs` |
| `inspect_vba` | _(none)_ | SHARED_PARTIAL | `operations.inspect_vba` | later | Capability-gated canonical VBA views | `crates/agent-spreadsheet-mcp/src/canonical_router.rs` | `crates/agent-spreadsheet-mcp/tests/canonical_projection.rs` |
| `write` | `write ...` | ALL | `operations.write` | mvp | Canonical preview/apply mutation batch in WASM; stage is invalid without durable bundles | `crates/agent-spreadsheet-mcp/src/canonical_router.rs` | `crates/agent-spreadsheet-mcp/tests/canonical_projection.rs` |
| `export_fork` | _(none)_ | MCP_ONLY | `operations.export_fork` | n/a | Canonical revision-bound fork export | `crates/agent-spreadsheet-mcp/src/canonical_router.rs` | `crates/agent-spreadsheet-mcp/tests/canonical_projection.rs` |
| `get_changes` | `verify diff` (partial overlap) | SHARED_PARTIAL | `operations.get_changes` | later | Canonical operation audit or net diff | `crates/agent-spreadsheet-mcp/src/canonical_router.rs` | `crates/agent-spreadsheet-mcp/tests/canonical_projection.rs` |
| `checkpoint` | _(none)_ | MCP_ONLY | `operations.checkpoint` | n/a | Canonical checkpoint action union | `crates/agent-spreadsheet-mcp/src/canonical_router.rs` | `crates/agent-spreadsheet-mcp/tests/canonical_projection.rs` |
| `staged_change` | _(none)_ | MCP_ONLY | `operations.staged_change` | n/a | Canonical staged-bundle action union | `crates/agent-spreadsheet-mcp/src/canonical_router.rs` | `crates/agent-spreadsheet-mcp/tests/canonical_projection.rs` |
| `list_workbooks` | _(none)_ | MCP_ONLY | `adapter-mcp.workspace.list_workbooks` | n/a | Workspace/repository concern | `crates/agent-spreadsheet/src/tools/mod.rs::list_workbooks` | `crates/agent-spreadsheet-mcp/tests/server_smoke.rs` |
| `describe_workbook` | `read workbook` | ALL | `core.read.describe_workbook` | mvp | Shared read primitive | `crates/agent-spreadsheet/src/tools/mod.rs::describe_workbook` | `crates/agent-spreadsheet-mcp/tests/server_smoke.rs` |
| `workbook_summary` | _(none direct)_ | SHARED_PARTIAL | `core.analysis.workbook_summary` | later | Candidate future CLI command | `crates/agent-spreadsheet/src/tools/mod.rs::workbook_summary` | `crates/agent-spreadsheet-mcp/tests/server_smoke.rs` |
| `list_sheets` | `read sheets` | ALL | `core.read.list_sheets` | mvp | Shared | `crates/agent-spreadsheet/src/tools/mod.rs::list_sheets` | `crates/agent-spreadsheet-mcp/tests/server_smoke.rs` |
| `sheet_overview` | `read overview` | ALL | `core.read.sheet_overview` | mvp | Shared | `crates/agent-spreadsheet/src/tools/mod.rs::sheet_overview` | `crates/agent-spreadsheet-mcp/tests/server_smoke.rs` |
| `sheet_page` | `read page` | ALL | `core.read.sheet_page` | mvp | Shared | `crates/agent-spreadsheet/src/tools/mod.rs::sheet_page` | `crates/agent-spreadsheet-mcp/tests/server_smoke.rs` |
| `find_value` | `analyze find-value` | ALL | `core.analysis.find_value` | mvp | Shared | `crates/agent-spreadsheet/src/tools/mod.rs::find_value` | `crates/agent-spreadsheet-mcp/tests/server_smoke.rs` |
| `read_table` | `read table` | ALL | `core.read.read_table` | mvp | Shared | `crates/agent-spreadsheet/src/tools/mod.rs::read_table` | `crates/agent-spreadsheet-mcp/tests/server_smoke.rs` |
| `table_profile` | `analyze table-profile` | ALL | `core.analysis.table_profile` | mvp | Shared | `crates/agent-spreadsheet/src/tools/mod.rs::table_profile` | `crates/agent-spreadsheet-mcp/tests/server_smoke.rs` |
| `range_values` | `read values` | ALL | `core.read.range_values` | mvp | Shared | `crates/agent-spreadsheet/src/tools/mod.rs::range_values` | `crates/agent-spreadsheet-mcp/tests/server_smoke.rs` |
| `inspect_cells` | `read cells` | ALL | `core.read.inspect_cells` | mvp | Strict detail-view (≤25 cells); returns budget metadata | `crates/agent-spreadsheet/src/tools/mod.rs::inspect_cells` | `crates/agent-spreadsheet-mcp/tests/read_guardrails_mcp.rs` |
| `sheet_statistics` | `analyze sheet-statistics` | ALL | `core.analysis.sheet_statistics` | mvp | Shared | `crates/agent-spreadsheet/src/tools/mod.rs::sheet_statistics` | `crates/agent-spreadsheet-mcp/tests/server_smoke.rs` |
| `sheet_formula_map` | `analyze formula-map` | ALL | `core.analysis.sheet_formula_map` | mvp | Shared | `crates/agent-spreadsheet/src/tools/mod.rs::sheet_formula_map` | `crates/agent-spreadsheet-mcp/tests/server_smoke.rs` |
| `formula_trace` | `analyze formula-trace` | ALL | `core.analysis.formula_trace` | later | Shared | `crates/agent-spreadsheet/src/tools/mod.rs::formula_trace` | `crates/agent-spreadsheet-mcp/tests/server_smoke.rs` |
| `named_ranges` | `read names` | ALL | `core.read.named_ranges` | mvp | Shared | `crates/agent-spreadsheet/src/tools/mod.rs::named_ranges` | `crates/agent-spreadsheet-mcp/tests/server_smoke.rs` |
| `define_name` | `write name define` | ALL | `core.write.define_name` | mvp | Named range CRUD (create) | `crates/agent-spreadsheet/src/tools/mod.rs::define_name` | `crates/agent-spreadsheet/tests/cli_integration.rs` |
| `update_name` | `write name update` | ALL | `core.write.update_name` | mvp | Named range CRUD (update) | `crates/agent-spreadsheet/src/tools/mod.rs::update_name` | `crates/agent-spreadsheet/tests/cli_integration.rs` |
| `delete_name` | `write name delete` | ALL | `core.write.delete_name` | mvp | Named range CRUD (delete) | `crates/agent-spreadsheet/src/tools/mod.rs::delete_name` | `crates/agent-spreadsheet/tests/cli_integration.rs` |
| `find_formula` | `analyze find-formula` | ALL | `core.analysis.find_formula` | mvp | Shared | `crates/agent-spreadsheet/src/tools/mod.rs::find_formula` | `crates/agent-spreadsheet-mcp/tests/server_smoke.rs` |
| `scan_volatiles` | `analyze scan-volatiles` | ALL | `core.analysis.scan_volatiles` | mvp | Shared | `crates/agent-spreadsheet/src/tools/mod.rs::scan_volatiles` | `crates/agent-spreadsheet-mcp/tests/server_smoke.rs` |
| `sheet_styles` | _(none)_ | SHARED_PARTIAL | `core.read.sheet_styles` | later | Candidate future CLI/WASM surface | `crates/agent-spreadsheet/src/tools/mod.rs::sheet_styles` | `crates/agent-spreadsheet-mcp/tests/unit_styles.rs` |
| `layout_page` | `read layout` | ALL | `core.read.layout_page` | mvp | Shared | `crates/agent-spreadsheet/src/tools/mod.rs::layout_page` | `crates/agent-spreadsheet-mcp/tests/server_smoke.rs` |
| `grid_export` | `read export --format grid` | ALL | `core.read.grid_export` | mvp | Shared | `crates/agent-spreadsheet/src/tools/mod.rs::grid_export` | `crates/agent-spreadsheet-mcp/tests/server_smoke.rs` |
| `workbook_style_summary` | _(none)_ | SHARED_PARTIAL | `core.analysis.workbook_style_summary` | later | Candidate future CLI/WASM surface | `crates/agent-spreadsheet/src/tools/mod.rs::workbook_style_summary` | `crates/agent-spreadsheet-mcp/tests/unit_workbook_style_summary_recalc.rs` |
| `get_manifest_stub` | `sheetport manifest candidates` | SHARED_PARTIAL | `core.sheetport.manifest_stub` | later | Shared semantic target | `crates/agent-spreadsheet/src/tools/mod.rs::get_manifest_stub` | `crates/agent-spreadsheet-mcp/tests/server_smoke.rs` |
| `execute_manifest` | `sheetport run`/`run-manifest` | ALL | `core.sheetport.execute_manifest` | later | Shared semantic target | `crates/agent-spreadsheet/src/tools/mod.rs::execute_manifest` | `crates/agent-spreadsheet-mcp/tests/server_smoke.rs` |
| `close_workbook` | _(none)_ | MCP_ONLY | `adapter-mcp.session.close_workbook` | n/a | Legacy 0.13 compat only; removed from canonical/default surface | `crates/agent-spreadsheet-mcp/src/server.rs` | `crates/agent-spreadsheet-mcp/tests/unit_mutate_batch.rs` |
| `vba_project_summary` | _(none)_ | SHARED_PARTIAL | `core.vba.project_summary` | later | Parser/runtime constraints for WASM | `crates/agent-spreadsheet/src/tools/vba.rs::vba_project_summary` | `crates/agent-spreadsheet-mcp/tests/unit_vba.rs` |
| `vba_module_source` | _(none)_ | SHARED_PARTIAL | `core.vba.module_source` | later | Same | `crates/agent-spreadsheet/src/tools/vba.rs::vba_module_source` | `crates/agent-spreadsheet-mcp/tests/unit_vba.rs` |
| `create_fork` | _(none)_ | MCP_ONLY | `adapter-mcp.fork.create` | n/a | MCP orchestration | `crates/agent-spreadsheet/src/tools/fork.rs::create_fork` | `crates/agent-spreadsheet-mcp/tests/fork_workflow.rs` |
| `edit_batch` | `write cells` | ALL | `core.write.edit_batch` | mvp | Shared write semantics | `crates/agent-spreadsheet/src/tools/fork.rs::edit_batch` | `crates/agent-spreadsheet-mcp/tests/fork_workflow.rs` |
| `mutate_batch` | _(none)_ | MCP_ONLY | `adapter-mcp.write.mutate_batch` | n/a | Consolidated previewable write surface; routes tagged ops to per-family batch handlers | `crates/agent-spreadsheet/src/tools/mutate_batch.rs::mutate_batch` | `crates/agent-spreadsheet-mcp/tests/unit_mutate_batch.rs` |
| `transform_batch` | `write batch transform` | ALL | `core.write.transform_batch` | mvp | Shared | `crates/agent-spreadsheet/src/tools/fork.rs::transform_batch` | `crates/agent-spreadsheet-mcp/tests/unit_transform_batch.rs` |
| `style_batch` | `write batch style` | ALL | `core.write.style_batch` | mvp | Shared | `crates/agent-spreadsheet/src/tools/fork.rs::style_batch` | `crates/agent-spreadsheet-mcp/tests/unit_style_batch.rs` |
| `grid_import` | `write import --from-grid` | ALL | `core.write.grid_import` | mvp | Shared | `crates/agent-spreadsheet/src/tools/fork.rs::grid_import` | `crates/agent-spreadsheet-mcp/tests/fork_workflow.rs` |
| `column_size_batch` | `write batch column-size` | ALL | `core.write.column_size_batch` | later | Shared | `crates/agent-spreadsheet/src/tools/fork.rs::column_size_batch` | `crates/agent-spreadsheet-mcp/tests/unit_column_size_batch.rs` |
| `sheet_layout_batch` | `write batch sheet-layout` | ALL | `core.write.sheet_layout_batch` | later | Shared | `crates/agent-spreadsheet/src/tools/sheet_layout.rs::sheet_layout_batch` | `crates/agent-spreadsheet-mcp/tests/unit_sheet_layout_batch.rs` |
| `apply_formula_pattern` | `write batch formula-pattern` | ALL | `core.write.apply_formula_pattern` | later | Shared | `crates/agent-spreadsheet/src/tools/fork.rs::apply_formula_pattern` | `crates/agent-spreadsheet-mcp/tests/unit_apply_formula_pattern.rs` |
| `structure_batch` | `write batch structure` | ALL | `core.write.structure_batch` | later | Shared | `crates/agent-spreadsheet/src/tools/fork.rs::structure_batch` | `crates/agent-spreadsheet-mcp/tests/unit_structure_batch.rs` |
| `rules_batch` | `write batch rules` | ALL | `core.write.rules_batch` | later | Shared | `crates/agent-spreadsheet/src/tools/rules_batch.rs::rules_batch` | `crates/agent-spreadsheet-mcp/tests/unit_rules_batch_cf.rs` |
| `replace_in_formulas` | `write formulas replace` | ALL | `core.write.replace_in_formulas` | later | Formula-only find/replace | `crates/agent-spreadsheet/src/tools/fork.rs::replace_in_formulas` | `crates/agent-spreadsheet-mcp/tests/unit_replace_in_formulas.rs` |
| `get_edits` | _(none)_ | MCP_ONLY | `adapter-mcp.fork.edit_log` | n/a | Fork audit trail | `crates/agent-spreadsheet/src/tools/fork.rs::get_edits` | `crates/agent-spreadsheet-mcp/tests/fork_workflow.rs` |
| `verify_workbook` | `verify proof` | ALL | `core.verify.compare_workbooks` | mvp | WASM compares current and baseline session resources in memory with the same proof contract | `crates/agent-spreadsheet/src/tools/mod.rs::verify_workbook` | `crates/agent-spreadsheet/tests/cli_integration.rs` |
| `get_changeset` | `verify diff` (partial overlap) | SHARED_PARTIAL | `core.diff.get_changeset` + adapter projection | later | MCP is fork diff, CLI is file diff | `crates/agent-spreadsheet/src/tools/fork.rs::get_changeset` | `crates/agent-spreadsheet-mcp/tests/fork_workflow.rs` |
| `recalculate` | `workbook recalculate` | SHARED_PARTIAL | `core.recalc.recalculate` | later | Backend constraints | `crates/agent-spreadsheet/src/tools/fork.rs::recalculate` | `crates/agent-spreadsheet-mcp/tests/unit_recalc_needed.rs` |
| `list_forks` | _(none)_ | MCP_ONLY | `adapter-mcp.fork.list` | n/a | MCP orchestration | `crates/agent-spreadsheet/src/tools/fork.rs::list_forks` | `crates/agent-spreadsheet-mcp/tests/fork_workflow.rs` |
| `discard_fork` | _(none)_ | MCP_ONLY | `adapter-mcp.fork.discard` | n/a | MCP orchestration | `crates/agent-spreadsheet/src/tools/fork.rs::discard_fork` | `crates/agent-spreadsheet-mcp/tests/fork_workflow.rs` |
| `save_fork` | _(none)_ | MCP_ONLY | `adapter-mcp.fork.save` | n/a | MCP orchestration | `crates/agent-spreadsheet/src/tools/fork.rs::save_fork` | `crates/agent-spreadsheet-mcp/tests/fork_workflow.rs` |
| `checkpoint_fork` | _(none)_ | MCP_ONLY | `adapter-mcp.fork.checkpoint_create` | n/a | MCP orchestration | `crates/agent-spreadsheet/src/tools/fork.rs::checkpoint_fork` | `crates/agent-spreadsheet-mcp/tests/fork_workflow.rs` |
| `list_checkpoints` | _(none)_ | MCP_ONLY | `adapter-mcp.fork.checkpoint_list` | n/a | MCP orchestration | `crates/agent-spreadsheet/src/tools/fork.rs::list_checkpoints` | `crates/agent-spreadsheet-mcp/tests/fork_workflow.rs` |
| `restore_checkpoint` | _(none)_ | MCP_ONLY | `adapter-mcp.fork.checkpoint_restore` | n/a | MCP orchestration | `crates/agent-spreadsheet/src/tools/fork.rs::restore_checkpoint` | `crates/agent-spreadsheet-mcp/tests/fork_workflow.rs` |
| `delete_checkpoint` | _(none)_ | MCP_ONLY | `adapter-mcp.fork.checkpoint_delete` | n/a | MCP orchestration | `crates/agent-spreadsheet/src/tools/fork.rs::delete_checkpoint` | `crates/agent-spreadsheet-mcp/tests/fork_workflow.rs` |
| `list_staged_changes` | _(none)_ | MCP_ONLY | `adapter-mcp.fork.staged_list` | n/a | MCP orchestration | `crates/agent-spreadsheet/src/tools/fork.rs::list_staged_changes` | `crates/agent-spreadsheet-mcp/tests/unit_staging.rs` |
| `apply_staged_change` | _(none)_ | MCP_ONLY | `adapter-mcp.fork.staged_apply` | n/a | MCP orchestration | `crates/agent-spreadsheet/src/tools/fork.rs::apply_staged_change` | `crates/agent-spreadsheet-mcp/tests/unit_staging.rs` |
| `discard_staged_change` | _(none)_ | MCP_ONLY | `adapter-mcp.fork.staged_discard` | n/a | MCP orchestration | `crates/agent-spreadsheet/src/tools/fork.rs::discard_staged_change` | `crates/agent-spreadsheet-mcp/tests/unit_staging.rs` |
| `screenshot_sheet` | `render` | ALL | `operations.screenshot_sheet` | mvp | Native raster renderer compiles to wasm32; WASM and just-bash render into a bounded session artifact slot and hand bytes across their own adapter boundary. LibreOffice stays an opt-in host-only backend | `crates/agent-spreadsheet/src/canonical_optional.rs::screenshot_sheet`, `crates/agent-spreadsheet-wasm/src/lib.rs::execute_in_memory_screenshot` | `crates/agent-spreadsheet-wasm/tests/session_screenshots.rs`, `crates/agent-spreadsheet-mcp/tests/canonical_projection.rs` |

---

## C) Canonical registry migration status

Wave 3A registers the canonical discovery/read/search/analysis surface. Wave 3B adds `write`, Wave 3C adds lifecycle/history/proof, and Wave 3D adds capability-gated rendering, SheetPort, and VBA operations. Wave 4 makes the descriptor-derived canonical router the default MCP surface; `SPREADSHEET_MCP_SLIM_SURFACE=false` adds the legacy 0.13 compatibility names and preserves legacy behavior for shared names. `asp operations`, `asp schema <operation>`, `asp op <operation>`, and MCP derive lookup and schemas from the registry.

| Canonical operation | Existing compatibility projection | Dispatcher implementation | Capability | Risk |
|---|---|---|---|---|
| `list_workbooks` | CLI/MCP `list_workbooks` remains legacy data-only | canonical discovery adapter; no request/resource binding | `workbook_discovery` | low |
| `describe_workbook` | legacy metadata and workbook-summary tools remain separate | exact metadata plus opt-in scoped derived summary | `workbook_read` | low |
| `list_sheets`, `sheet_overview`, `read_table` | CLI/MCP wrappers project canonical `data` | shared semantic implementations | `workbook_read` | low |
| `read_cells` | legacy `range_values` and `sheet_page` remain available | correlated range/row engine with revision/request-bound opaque cursor | `workbook_read` | low |
| `inspect_cells` | legacy detail view stays separate to retain its historical partial-payload behavior; canonical fails before truncation | shared semantic implementation with distinct truncation policy | `workbook_read` | low |
| `named_ranges` | legacy MCP wrapper projects canonical `data` | shared semantic implementation | `workbook_read` | low |
| `read_layout` | legacy `layout_page` stays separate | explicitly lossy layout projection | `workbook_read` | low |
| `export_grid` | legacy `grid_export` stays separate | coordinate-preserving paged grid export of cell content and explicit formatting; implicit presentation defaults excluded | `workbook_read` | low |
| `analyze_styles` | legacy sheet/workbook style tools stay separate | closed `scope.kind` union with bounded-count coverage | `workbook_read` | low |
| `search_values` | legacy `find_value` projects canonical `data` | preserves label, direction, scope, type, header, and context options | `workbook_read` | low |
| `search_formulas` | legacy formula search/volatile tools stay separate | closed `result_mode` branches and actual function classifications | `workbook_read` | low |
| `formula_trace` | legacy structured-cursor wrapper stays separate; canonical uses a revision/request-bound opaque cursor | shared formula analysis |
| `formula_map` | legacy MCP wrapper projects canonical `data` | shared formula analysis with opaque canonical continuation | `workbook_read` | low |
| `profile_table`, `sheet_statistics` | legacy MCP wrappers project canonical `data` | shared bounded analysis | `workbook_read` | low |
| `write` | `edit_batch`, `mutate_batch`, and family tools remain compatibility surfaces | one ordered dispatcher over the existing family implementations; pure preview, CAS, atomic apply, explicit partial apply, and one-bundle stage | `workbook_write` | destructive ceiling; request-aware moderate/high/destructive |
| `create_fork`, `list_forks`, `discard_fork` | legacy fork tools remain unchanged | typed resource discovery/lifecycle with no path-bearing canonical responses; create/discard use revision validation | `workbook_write` | moderate/low/destructive |
| `recalculate`, `verify_workbook` | legacy proof tools remain unchanged | canonical recalc uses copy-evaluate-CAS-swap; verification preserves the F1 state, coverage, and proof contract for both resources | `workbook_write` | high/low |
| `export_fork` | legacy `save_fork` retains path-oriented MCP adapter UX | workspace destination plus portable artifact metadata; canonical data never relies on a server-local path | `workbook_write` | high |
| `get_changes` | legacy `get_edits`/`get_changeset` remain unchanged | honest `view.kind` union: stored canonical operation audit or direct fork-base net diff | `workbook_write` | low |
| `checkpoint` | four legacy checkpoint tools remain unchanged | action union with CAS mutations; atomic restore reports removed operations/stages and retained/invalidated checkpoints | `workbook_write` | destructive ceiling; request-aware low/moderate/high/destructive |
| `staged_change` | three legacy staging tools remain unchanged | action union; canonical bundles bind applicability to base content revision, replay through the write dispatcher, and are consumed only after success | `workbook_write` | destructive ceiling; request-aware low/moderate/destructive |
| `screenshot_sheet` | legacy MCP screenshot tool remains unchanged | validates bounded names/ranges before rendering, selects `native` or `libreoffice` with an optional `png_level`, and returns a content-addressed PNG handle plus renderer identity, fidelity, warnings, geometry, and calculation state without server paths; every adapter hands the bytes across its own boundary | `screenshot_rendering` | low |
| `sheetport_manifest` | legacy CLI hierarchy and `get_manifest_stub` remain unchanged | closed action union; schema/validate/normalize are unbound portable content actions, while candidates/bind_check require a resource | `sheetport` | low |
| `execute_sheetport` | legacy `sheetport run` and `execute_manifest` remain unchanged | bounded typed values with declared/returned coverage, typed status/error codes, and structured missing-input/constraint failures | `sheetport` | low |
| `inspect_vba` | legacy VBA summary/source tools remain unchanged | closed bounded views with opaque cursors bound to resource, request fingerprint, and workbook revision; stream names, debug bodies, and paths are omitted | `vba` | low |

Canonical responses use the versioned operation envelope and state reads carry `revision_id`. Value-bearing reads expose calculation state. Merged responses echo branch discriminants. Checked-in full JSON fixtures cover every branch, action, and view, including deterministic screenshot artifact tokens.

Compatibility projections are only routed through the dispatcher when the legacy response can be reconstructed without loss. `range_values`/`sheet_page`, workbook summaries, layout/grid, style summaries, and formula-search variants remain separate compatibility implementations rather than claiming false response parity. Existing write and lifecycle tools retain their response/error projections while sharing the canonical state and family implementations where response compatibility permits. Canonical export keeps path-oriented CLI/MCP/WASM concerns in adapters. Wave 4 authorizes the canonical MCP default while preserving the 0.13 router only in explicit compatibility mode.

---

## D) Enforcement hooks

- Boundary contract (non-negotiable): `docs/architecture/surface-boundary-rules.md`
- Matrix drift checker: `scripts/check_surface_matrix_drift.py`
- Local/CI invocation:
  - `python3 scripts/check_surface_matrix_drift.py`
  - `cargo test -p agent-spreadsheet surface_matrix_drift_check`
