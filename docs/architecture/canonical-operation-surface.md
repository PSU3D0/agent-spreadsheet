# Canonical Operation Surface

Status: contract v1 frozen after Wave 0 / Fable Gate 1

This document defines the target semantic operation surface shared by the Rust core, CLI, MCP server, WASM bindings, JavaScript SDK, and future just-bash adapter.

The objective is not the smallest possible tool count. It is **one durable operation per distinct agent intent**, with consolidation only where existing tools differ because of pagination, formatting, CRUD/module boundaries, or one-vs-many inputs.

## Decision criteria

Keep operations separate when they differ in:

- the question the caller is asking;
- response shape or interpretation;
- expected cost, paging, or budget behavior;
- safety or approval implications;
- position in the edit/recalculate/prove workflow.

Consolidate operations when they differ only in:

- exact-range versus row-window selection;
- output formatting over the same data;
- one versus many edits;
- CRUD actions over one resource with the same identity model;
- the Rust module that currently implements the behavior.

Examples:

- `sheet_overview` remains separate from cell reads: it answers "what is this sheet?" using derived structural analysis.
- `inspect_cells` remains separate from bulk cell reads: it provides bounded, sparse diagnostic metadata for exact cells.
- `sheet_page` and `range_values` become `read_cells`: both are paged bulk extraction with compatible output semantics.
- value and formula search remain explicit search operations rather than modes on `read_cells`.
- all write families compile to a single `write` operation with one or many tagged ops.

## Architecture boundary

Semantic behavior must live behind one Rust dispatcher:

```text
CLI parser ───────────────┐
MCP tool wrappers ────────┤
WASM JSON binding ────────┼── execute_operation(context, request)
SDK backend/transport ────┤
just-bash adapter ────────┘
```

A canonical registry describes each operation:

```rust
pub struct OperationDescriptor {
    pub name: &'static str,
    pub description: &'static str,
    pub capability: Capability,
    pub risk_ceiling: OperationRisk,
    pub risk_for: fn(&SpreadsheetOperation) -> OperationRisk,
    pub capability: fn(&RuntimeCapabilities) -> bool,
    pub cost: OperationCost,
    pub schema_version: &'static str,
    pub input_schema: fn() -> RootSchema,
    pub output_schema: fn() -> RootSchema,
}

pub enum SpreadsheetOperation {
    ListWorkbooks(ListWorkbooksRequest),
    DescribeWorkbook(DescribeWorkbookRequest),
    ReadCells(ReadCellsRequest),
    SheetOverview(SheetOverviewRequest),
    Write(WriteRequest),
    Recalculate(RecalculateRequest),
    VerifyWorkbook(VerifyWorkbookRequest),
    // ...
}
```

The registry is the source for operation names, schemas, descriptions, capability flags, risk/approval classification, CLI machine-mode discovery, and generated SDK types/methods. Surface adapters may add transport and ergonomic parsing, but not spreadsheet behavior or independent normalization rules.

## Target default surface

### Discovery and reading

| Operation | Intent | Consolidation |
|---|---|---|
| `list_workbooks` | Discover workbook resources available to the runtime | unchanged |
| `describe_workbook` | Workbook metadata, capabilities, and high-level summary | absorbs `workbook_summary` |
| `list_sheets` | Cheap sheet inventory and bounds | unchanged |
| `sheet_overview` | Regions, likely tables, headers, bounds, and notable structure | unchanged |
| `read_cells` | Bulk rectangular or row-window extraction with pagination | merges `range_values` + `sheet_page` |
| `inspect_cells` | Sparse deep diagnostics: value, formula, cache, type, format, style | unchanged |
| `read_table` | Header-aware tabular/record extraction | unchanged |
| `read_layout` | Lossy display/layout representation for human or agent orientation | renames `layout_page` |
| `export_grid` | Cell content and explicit-format grid payload; workbook/sheet presentation defaults are not preserved | renames `grid_export`; pairs with `write.import_grid` |
| `named_ranges` | Read workbook- and sheet-scoped names | unchanged |
| `analyze_styles` | Workbook- or sheet-scoped style patterns | merges `sheet_styles` + `workbook_style_summary` |

`read_cells` uses one selection contract:

```json
{
  "resource_id": "...",
  "sheet_name": "Revenue",
  "selection": {
    "kind": "range",
    "ranges": ["A1:F100"]
  },
  "include_formulas": true,
  "format": "dense",
  "page_size": 50,
  "cursor": null
}
```

A row-window request changes only `selection`:

```json
{
  "selection": {
    "kind": "rows",
    "start_row": 101,
    "row_count": 50
  }
}
```

The response has one canonical block/page envelope and continuation cursor regardless of selection kind.

### Search and analysis

| Operation | Intent | Consolidation |
|---|---|---|
| `search_values` | Search literal values/text across workbook scope | renames `find_value` |
| `search_formulas` | Search formula bodies and classifications | absorbs `scan_volatiles` as a filter |
| `formula_trace` | Trace upstream/downstream dependencies from a formula target | unchanged |
| `formula_map` | Map sheet-level formula topology and patterns | renames `sheet_formula_map` |
| `profile_table` | Profile columns, types, distributions, and data quality | renames `table_profile` |
| `sheet_statistics` | Compute bounded sheet-level statistics | unchanged |

Volatile discovery becomes:

```json
{
  "resource_id": "...",
  "filter": { "volatile": true }
}
```

This preserves the capability without assigning it a separate tool identity.

### Mutation and proof

| Operation | Intent | Consolidation |
|---|---|---|
| `create_fork` | Create isolated mutable workbook state | unchanged |
| `list_forks` | Recover/discover active working states | unchanged |
| `write` | Preview or apply one or many tagged mutations | absorbs all write-family tools |
| `recalculate` | Evaluate formulas and report evaluation coverage/errors | unchanged; must include F1 soundness work |
| `verify_workbook` | Compare baseline/current states and prove effects | unchanged |
| `export_fork` | Persist a fork to a filesystem/workspace destination | renames `save_fork` |
| `discard_fork` | Explicitly destroy working state | unchanged |

`write` is the canonical successor to both `edit_batch` and `mutate_batch`:

```json
{
  "resource_id": "fork:fork-...",
  "expected_revision": "...",
  "mode": "preview",
  "atomic": true,
  "ops": [
    {
      "kind": "set_cells",
      "sheet_name": "Revenue",
      "cells": {
        "B4": { "kind": "value", "value": 1500 },
        "C4": { "kind": "formula", "formula": "=B4*1.2" }
      }
    },
    {
      "kind": "insert_rows",
      "sheet_name": "Revenue",
      "at_row": 5,
      "count": 2
    }
  ]
}
```

Its tagged op union includes:

- cell/value/formula writes;
- clear/fill/matrix transforms;
- row, column, sheet, merge, copy, and move structure changes;
- style, column-size, sheet-layout, rule, and validation changes;
- formula patterns and formula-body replacement;
- named-range create/update/delete;
- rich grid/CSV imports where supported.

One op and many ops use the same request. Pure preview reports `ops_previewed` and creates no staged state; stage reports `ops_staged`; apply reports `ops_applied`. Atomic failure rolls back completely. Explicit non-atomic execution identifies every applied, failed, and skipped op.

`recalculate` and `verify_workbook` stay separate because evaluation and comparison are distinct intents. `recalculate` may optionally request proof against a baseline, but this convenience composes the same verification implementation rather than replacing `verify_workbook`.

### History and safety

| Operation | Intent | Consolidation |
|---|---|---|
| `get_changes` | Read summarized or detailed fork history | merges `get_edits` + `get_changeset` |
| `checkpoint` | Create/list/restore/delete checkpoints | merges four checkpoint tools |
| `staged_change` | List/apply/discard staged changes | merges three staged-change tools |

These action unions are appropriate because they operate on one resource identity and share compact schemas. Risk classification remains action-specific so destructive restore/delete/apply operations can require approval.

### Optional capability groups

These operations are registered only when their backing capability is enabled. The Wave 3D registry uses separate `screenshot_rendering`, `sheetport`, and `vba` predicates; discovery omits unavailable operations rather than advertising a callable stub.

| Operation | Capability |
|---|---|
| `screenshot_sheet` | LibreOffice/image rendering |
| `sheetport_manifest` | Discover/get/validate a SheetPort manifest |
| `execute_sheetport` | Execute a typed SheetPort interface |
| `inspect_vba` | VBA project summary or bounded module source |

`asp op` binds and injects `resource_id` only for operations that consume a workbook. Within `sheetport_manifest`, `candidates` and `bind_check` require `--bind`; `schema`, `validate`, and `normalize` reject resource binding and run against portable content. Optional capability overrides are intersected with compile-time and runtime backing and cannot advertise an unavailable implementation.

## Operations removed from the agent surface

- `close_workbook`: cache eviction is runtime administration and should be automatic/out-of-band.
- Individual transform/style/structure/layout/rules/column-size write tools.
- Separate name create/update/delete tools.
- Separate `grid_import`, formula-pattern, and formula-replace tools.
- Separate checkpoint CRUD tools.
- Separate staged-change lifecycle tools.
- Separate workbook/sheet style-analysis tools.
- `scan_volatiles` as a special search operation.
- Both `get_edits` and `get_changeset`.

## Current-to-canonical mapping

| Current operation(s) | Canonical operation |
|---|---|
| `list_workbooks` | `list_workbooks` |
| `describe_workbook`, `workbook_summary` | `describe_workbook` |
| `list_sheets` | `list_sheets` |
| `sheet_overview` | `sheet_overview` |
| `sheet_page`, `range_values` | `read_cells` |
| `inspect_cells` | `inspect_cells` |
| `read_table` | `read_table` |
| `layout_page` | `read_layout` |
| `grid_export` | `export_grid` |
| `named_ranges` | `named_ranges` |
| `sheet_styles`, `workbook_style_summary` | `analyze_styles` |
| `find_value` | `search_values` |
| `find_formula`, `scan_volatiles` | `search_formulas` |
| `formula_trace` | `formula_trace` |
| `sheet_formula_map` | `formula_map` |
| `table_profile` | `profile_table` |
| `sheet_statistics` | `sheet_statistics` |
| `edit_batch`, `mutate_batch`, `transform_batch`, `style_batch`, `structure_batch`, `column_size_batch`, `sheet_layout_batch`, `rules_batch`, `apply_formula_pattern`, `replace_in_formulas`, `grid_import`, `define_name`, `update_name`, `delete_name` | `write` |
| `create_fork`, `list_forks`, `discard_fork` | unchanged |
| `save_fork` | `export_fork` |
| `recalculate` | `recalculate` |
| `verify_workbook` | `verify_workbook` |
| `get_edits`, `get_changeset` | `get_changes` |
| `checkpoint_fork`, `list_checkpoints`, `restore_checkpoint`, `delete_checkpoint` | `checkpoint` |
| `list_staged_changes`, `apply_staged_change`, `discard_staged_change` | `staged_change` |
| `screenshot_sheet` | `screenshot_sheet` (optional) |
| `get_manifest_stub` | `sheetport_manifest` (optional) |
| `execute_manifest` | `execute_sheetport` (optional) |
| `vba_project_summary`, `vba_module_source` | `inspect_vba` (optional) |
| `close_workbook` | remove from agent surface |

Target: approximately **27 default operations and 31 fully enabled operations**, down from 46 in the current slim MCP surface, without collapsing distinct agent intents.

## Normative cross-cutting contracts (Gate 1)

This section resolves Wave 0's five blocking contract questions and overrides any earlier illustrative sketch that conflicts with it. The later [resident-session contract](resident-session-runtime.md) narrowly extends explicit session binding, shared persistence ownership, workbook-versus-catalog revisions, and non-copying atomic implementation choices. Existing ephemeral file semantics remain unchanged; new capabilities are advertised only after implementation.

### Resource binding and identity

Canonical operations receive exactly one opaque `resource_id`. Workbook, fork, and session resources share a typed-prefix namespace. The semantic operation layer never receives a filesystem path, raw workbook bytes, or a just-bash VFS path.

Adapters own binding and export:

- MCP discovery/fork lifecycle resolves workspace entries to resource ids.
- CLI human commands remain stateless and path-driven. Canonical machine mode binds `--bind <path>` to an ephemeral resource. Mutations use an ephemeral fork and export/replace atomically according to CLI adapter flags.
- WASM `createSession(bytes)` returns a resource id; export remains a byte/session adapter operation.
- just-bash reads a VFS path through `ctx.fs`, enforces byte limits, creates the same WASM session, dispatches by resource id, and exports through `ctx.fs`.

Any current CLI behavior that cannot compile to bind → canonical operations → export is either adapter-only file UX or a missing canonical write op. `append_rows`, `clone_row`, and `clone_row_band` are therefore canonical write op kinds rather than CLI-owned spreadsheet planners.

All state-reading responses carry `revision_id`. Mutations require `expected_revision` and return `revision_before` / `revision_after`. A mismatch is `REVISION_CONFLICT` with zero effects. Cursors are opaque, bound to the resource revision and request fingerprint, and fail with `STALE_CURSOR` or `CURSOR_MISMATCH` rather than silently continuing against changed data.

### Evaluation and proof soundness

`recalculate` reports:

```json
{
  "state": "clean | errors_found | partial | not_evaluated",
  "evaluation_coverage": {
    "formula_cells": 0,
    "evaluated_formula_cells": 0,
    "unsupported_formula_cells": 0,
    "error_formula_cells": 0,
    "source": "formualizer | trusted_cache | none",
    "freshness": "current_revision | stale | unknown",
    "revision_id": "..."
  }
}
```

Cache presence alone never authorizes `clean`; only complete trusted evaluation for the same revision does. Structural formula ratio is `formula_cells / occupied_cells`, bounded to `[0,1]`, and is independent of evaluation coverage.

`verify_workbook` evaluates both sides in memory by default. Its proof status is `proved | differences_found | inconclusive_unevaluated | failed`. Empty error arrays and clean/proved claims are unreachable without complete fresh coverage. Asymmetric coverage cannot manufacture new/resolved-error provenance.

Value-bearing reads (`read_cells`, `read_table`, `inspect_cells`) include lightweight `calculation: {state, revision_id}` metadata. They do not perform evaluation unless explicitly requested, but they never present stale/unknown values without disclosing known calculation state.

The Wave 1 compatibility responses use these additive field names:

- `recalculate`: `state` and `evaluation_coverage` are added beside the existing backend, timing, telemetry, and error fields. A timeout/cancellation is a normal `partial` response and does not persist incomplete caches.
- `verify_workbook`: `proof_status`, `baseline_state`, `current_state`, `baseline_evaluation_coverage`, and `current_evaluation_coverage` are added beside the existing deltas and summary. The default proof path evaluates temporary workbook copies with Formualizer and never mutates either bound resource. When resource identity and revision hash match, one evaluated copy is reused for both sides. Distinct resources still incur a temporary-file write, backend parse/evaluation/write, and evaluated-file parse per side; removing that backend cost requires a future in-memory evaluator adapter. `failed` includes a `failure` message and empty delta arrays because asymmetric or failed evaluation cannot support error provenance.
- Compatibility `range_values`, `read_table`, and `inspect_cells`: `calculation.state` uses the same four-state vocabulary and `calculation.revision_id` identifies the values read. Imported caches can establish diagnostic partial coverage or known errors, but unknown freshness remains `not_evaluated` rather than `clean`. Reads do not retain a prior operation's evaluation proof across independent resource bindings.

### Write atomicity, concurrency, and staging

`write.mode` is `preview | apply | stage`:

- `preview` is pure: validate and compute impact/diff, create no durable state, mutate nothing.
- `apply` mutates the target.
- `stage` creates exactly one ordered approval bundle with its `base_revision`; it is not the default cautious workflow.

`atomic` defaults to `true`. Every request fully parses and statically validates all ops before mutation. Atomic execution uses a temporary workbook/state and atomic swap, or validated bounded prepared deltas/inverses for resident operations under the shared recovery contract: failure leaves the original state/revision unchanged and reports `rolled_back: true`. `atomic:false` is explicit; partial execution returns `status:"partial"`, exact applied/skipped counts, and one result per op. A side-effecting call never communicates partial effects only through a transport error.

Every mutation requires `expected_revision`. Preview → apply is safe because apply uses the previewed revision as CAS. Staged apply requires `base_revision == current workbook state revision`; stale bundles are rejected, not implicitly rebased. In resident mode creating/discarding approvals advances a separate catalog generation, not the workbook revision, so immediate stage/apply and alternative bundles remain coherent. A published calculation or other workbook revision transition invalidates older approvals. Staging requires `atomic:true`; unsupported non-atomic staging is rejected rather than recorded with misleading semantics. Every write op kind is stageable/applicable through the same dispatcher.

Canonical write responses are a closed union keyed by top-level `status`: `previewed`, `staged`, `applied`, `partial`, `failed`, or `rolled_back`. Per-op failures carry structured `code`, `message`, `path`, and `retryable` fields. `asp op write` exits 0 for complete preview/stage/apply, 1 for `failed` or `rolled_back`, and 2 for `partial`; partial output is exported before exit so the response and workbook agree.

The `write` schema is a closed discriminated union, not `{kind, additionalProperties:true}`. `set_cells` uses typed content (`value` versus `formula`) rather than inferring formula meaning from a leading equals sign. The union covers all existing families plus `set_cells`, `import_grid`, `import_csv`, named-range CRUD, `append_rows`, `clone_row`, and `clone_row_band`.

### Merge admission, response discriminants, and omissions

A merged operation is admitted only when:

1. its request has a required top-level discriminant (`selection.kind`, `view.kind`, `scope.kind`, `result_mode`, or `action`) echoed by the response;
2. it has one closed stable envelope with branch-specific payload keys;
3. every branch offers compatible paging, truncation, and losslessness guarantees;
4. registry input/output schemas and golden fixtures exist per branch.

This is why `read_cells` remains merged but `read_layout` and `export_grid` are separate: display layout is intentionally lossy while grid export preserves cell content and explicit formatting without claiming preservation of implicit presentation defaults.

Required discriminants include:

- `read_cells.selection.kind` (`range | rows`);
- `search_formulas.result_mode` (`cells | groups`);
- `analyze_styles.scope.kind` (`workbook | sheet`);
- `get_changes.view.kind` (`operations | net_diff`);
- `checkpoint.action`;
- `staged_change.action`;
- `sheetport_manifest.action`;
- `inspect_vba.view`.

Expensive fields are opt-in. Omitted, `null`, and empty have distinct meanings. Derived/inferred values never flatten into exact metadata namespaces. `describe_workbook` returns cheap exact metadata by default; `include:["summary"]` adds a separately scoped summary with coverage/status.

`read_cells` returns correlated blocks (`selection_index`, requested/returned ranges) and one revision-bound opaque continuation cursor. A single small exact range remains simple: one complete block and `next_cursor:null`. Row-window selection retains column projection by letters/headers, header inclusion, style tags, formulas, and current encodings.

`export_grid` preserves cell content, explicit cell formatting, absolute coordinates, and merge-repeat semantics without trimming. It does not preserve workbook/sheet default presentation (including implicit default column widths), so it explicitly reports `fidelity: cell_content_and_explicit_formatting` and does not claim a lossless presentation round trip. `read_layout` may trim/cap/render for display and labels itself lossy.

### Risk and capability metadata

The registry has a static risk ceiling plus `risk_for(request)`. Hosts with per-request policy use the dynamic result. MCP annotations are static and use the worst-case risk for a union tool. A union is acceptable only when its common low-risk use is not harmed by worst-case destructive approval friction; read/write unions are forbidden.

Capability registration is adapter-specific. `list_workbooks`, rendering, SheetPort, VBA, and fork lifecycle are registered only where backed. Current SDK capability claims must reflect actual registered/implemented behavior; unsupported WASM verification methods must fail explicitly rather than echoing apparent success.

All canonical responses include `schema_version`. Errors use one structured envelope. Warnings are structured data. Optional screenshot results use bounded artifact handles, never server-local paths as the only result.

## Surface projections

### CLI

The existing ergonomic hierarchy may remain as aliases. Add a canonical machine entrypoint:

```bash
asp op read_cells --json '{...}'
echo '{...}' | asp op write
asp operations
asp schema read_cells
```

`asp op` performs only operation lookup, JSON decoding, core dispatch, and canonical JSON/error serialization. It is the parity reference for non-native adapters.

### MCP

Each default canonical operation is an individual MCP tool with its own narrow input schema and description. Optional groups remain conditionally registered. Legacy tools stay available only in explicit compatibility mode during migration.

### WASM

Expose a single operation dispatch binding plus resource/session lifecycle needed for workbook bytes:

```text
executeOperation(sessionId, operationName, paramsJson)
```

Do not independently implement one binding method per operation unless generated from the canonical registry.

### JavaScript SDK

The SDK becomes a typed facade over one backend dispatch contract:

```js
backend.execute("read_cells", input)
backend.readCells(input) // generated convenience wrapper
```

Remove handwritten per-method response normalization where canonical responses can be emitted by the Rust core. Backend differences are transport/resource concerns represented by explicit capabilities, not different operation semantics.

### just-bash

The adapter registers one `asp` custom command and supports the canonical machine protocol using `ctx.fs`:

```text
asp op <operation> --json <payload>
```

It must not define an operation list or argument schemas independently. Operations/help/schema are read from the SDK/core registry. Spreadsheet WASM executes in the trusted host adapter, not inside just-bash's QuickJS worker. The production adapter should remain under approximately 150 lines excluding tests and virtual-filesystem plumbing; exceeding that is evidence of behavior leaking out of the core.

## Compatibility and migration

1. Add canonical operations alongside existing tools; no immediate removals.
2. Route old operations through canonical implementations and emit structured deprecation metadata only in verbose/discovery output, not noisy per-call warnings.
3. Make the new canonical set the default slim MCP router.
4. Retain `SPREADSHEET_MCP_SLIM_SURFACE=false` (or a renamed compatibility flag) for one documented compatibility window.
5. Preserve human-oriented CLI commands as aliases over canonical requests.
6. Deprecate SDK methods only after generated canonical equivalents are available.
7. Remove compatibility wrappers in a semver-signaled release after telemetry/issues show no required gaps.

## Acceptance gates

- One Rust implementation and canonical request/response type per operation.
- CLI machine mode, MCP, WASM, and SDK produce semantically identical JSON for shared fixtures.
- Cross-surface parity tests are generated from operation descriptors and golden fixtures.
- Canonical default MCP surface is about 27 tools; optional full surface about 31.
- Tool-list bytes and initialization instructions are measured and remain below the 0.13 slim baseline.
- `read_cells` empirically covers exact-range and row-window workflows without loss of continuation metadata.
- `write` covers every current mutation family, including simple cell edits, with preview/apply and indexed failure semantics.
- F1: recalculation reports evaluation coverage; verification never classifies unevaluated formulas as clean.
- Existing SOL agent traces are rerun against canonical tools; a read-only Fable review checks selection clarity, response claims, and unnecessary turns.
- The just-bash adapter contains no spreadsheet behavior and passes native `asp op` JSON golden tests.
