# agent-spreadsheet-wasm

`agent-spreadsheet-wasm` is the byte-session adapter for the shared canonical operation dispatcher. It does not expose workspace paths, forks, or host process assumptions.

## Canonical API

```js
const sessionId = createSession(workbookBytes); // session:<opaque-id>
const resultJson = await executeOperation(
  sessionId,
  "read_cells",
  JSON.stringify({
    sheet_name: "Sheet1",
    selection: { kind: "range", ranges: ["A1:C20"] }
  })
);
const outputBytes = exportWorkbook(sessionId);
disposeSession(sessionId);
```

`paramsJson` uses the canonical operation schema. The adapter injects `resource_id` when omitted; if supplied, it must exactly match `sessionId`. Success is a serialized canonical response envelope. Rejections from `executeOperation` are canonical error envelopes.

Call `operations()` for serialized canonical discovery filtered to capabilities actually backed by this runtime. The byte-session runtime supports the advertised canonical reads plus `write`, `recalculate`, and `verify_workbook`. Writes use revision CAS; preview is pure, apply atomically replaces the session bytes by default, and non-atomic apply returns canonical partial results. `mode:"stage"` is rejected with `INVALID_REQUEST` because WASM sessions do not expose durable staged bundles. Recalculation uses Formualizer entirely in memory, and verification binds the current session to another `session:` id supplied as `baseline_resource_id`.

Workspace discovery, fork/checkpoint/history lifecycle, LibreOffice rendering, SheetPort, and VBA inspection remain hidden. `exportWorkbook(sessionId)` always returns the latest applied or recalculated bytes.

The older named bindings remain for compatibility. New integrations should use `executeOperation`; `exportWorkbook` and `disposeSession` own the adapter lifecycle.

## Original-source XLSX cache recalculation

Call this before `createSession` when repairing source-package caches. It sends the supplied bytes directly to Formualizer's source API, without importing/saving through Umya or allocating a session. Resident editing and canonical session recalculation keep their existing contracts.

```js
const repaired = recalculateSourceXlsx(originalBytes, JSON.stringify({
  errorLocationLimit: 3,
  externalLinks: "cached",
  limits: { maxFormulaCells: 200000, maxColumns: 1024 }
}));
const outputBytes = repaired.bytes; // Uint8Array, separately from metadata
const metadata = JSON.parse(repaired.metadataJson);
repaired.free(); // releases the Rust result allocation
```

The synchronous binding returns bytes only on success. Unchanged results retain exact bytes; byte equality does not establish freshness. Changed caches preserve unrelated package members. `metadata.summary` contains exact `evaluated` and `errors` totals, per-sheet totals, `errorSummary` counts and sampled `locations`/`messages` with `locationsTruncated`, and `unknownFunctions`. Excel error values are successful data (`status: "errors_found"`), separate from a thrown admission/parse/unsupported/cancellation/operational error. Failures throw `{code,message}` with the upstream reason and return no result bytes. Physical `cacheCellsChanged` and `worksheetPartsChanged` are distinct from engine deltas; `clockNowUtc` is the effective RFC3339 clock and `externalLinksUsed` counts cached links read.

Options are strict camelCase JSON: unknown fields fail. `errorLocationLimit` controls sampling only; `externalLinks` accepts `"cached"` (default) or `"refuse"`. Neither refreshes external links. `limits` accepts all upstream bounds: `maxInputBytes`, `maxEntries`, `maxExpandedBytes`, `maxWorksheetBytes`, `maxFormulaCells`, `maxOutputBytes`, `maxXmlDepth`, `maxCells`, `maxColumns`, `maxOperationRoots`, `maxOperationTextBytes`, `maxAffectedWork`, `maxAffectedRegions`, `maxAffectedScratchBytes`. Canonical `SourceRecalculateLimits` decoding requires finite nonnegative integers: explicit null, booleans, fractions, unknown fields and out-of-domain values fail. Byte/count limits fit wasm32, columns cannot exceed 16,384, and affected work is a JS-safe integer. The selected profile is constructed before partial overrides are applied. Omitted values use upstream defaults (currently 64 MiB input/output, 10,000 entries, 256 MiB expanded, 128 MiB per worksheet, 100,000 formulas, XML depth 128, 8,000,000 cells, 16,384 columns for the workbook profile (256 for strict), 20 sampled locations per error kind). These are configurable admission bounds, not universal engine capacity. The adapter independently caps input/output at 64 MiB and options JSON at 1 MiB. Its inherited output guard is applied to the effective upstream output limit before package construction: raising maxOutputBytes cannot bypass that adapter ceiling. This is an adapter capacity guard, not a universal Node/WASM or engine limit.

File-facing requests default to `profile:"workbook"`; explicit `profile:"strict"` preserves legacy guards. `requireComplete` is independent. `calculation` deserializes the upstream portable policy, including iteration, cycle detection, exact u64 seed strings/safe integers, RFC3339 fixed clock, timezone, budgets and require_verified/cold_start iteration assumptions. See [source-runtime.md](../../plugins/formualizer/skills/spreadsheet/references/source-runtime.md) for the schema. Raw EvalConfig and cancellation tokens remain native-only. Native option defaults remain strict. A host may terminate its worker to interrupt execution. Native Rust callers use `agent_spreadsheet::recalc::source_xlsx::recalculate_source_xlsx_bytes(&bytes, XlsxRecalculateOptions)` with the full upstream EvalConfig, cancellation token and limits. This byte API performs no shared-file publication or CAS; the caller owns publication.

Successful metadata includes shared upstream effective `calculation`, `coverage` (complete/partial), `coverageScope`, `cachesInvalidated`, `calculationFlagsChanged`, and `operationDiagnostics` (including complete `unavailableFunctions` incidence, `unavailableResultCells`, and `workbookFeatures` independent of samples). The flag-edit receipt is a known boolean for completed source publication and null on deferral; skipped edits have no recalculation metadata. `functionModes` distinguishes unavailable from unverified executed modes; typed operation samples distinguish unknown functions, unimplemented/unverified modes and unverified providers. `dependencyEvidence` describes conservative admitted-workbook/fixed-cache relations and explicitly leaves semantic reads unattested. `physicalTelemetry:true` opts into physical observations/dispatches; omitted telemetry is null, measured zero is 0, and counters above the JS-safe range serialize as exact decimal strings. These are separate from logical roots, final Excel errors, invalidated caches and physical cache patches. Sampling zero does not disable evidence or its finite resource bounds. Coverage is a known-gap claim under declared assumptions, not Excel numerical equivalence. `attempted`, `completed`, `artifactAvailable` and `bytesUnchanged` describe independent facts.

Only typed CalculationDeferred in workbook profile with requireComplete:false returns input/valid edited bytes with deferred coverage, completed:false, a reason/context and null unknown counts/summary. Deferred bytes make no cache freshness claim. Strict/requireComplete and resource, cancellation, output, corrupt-package and options failures remain errors.

## Stateless batch editing

Compact requests skip full package diff construction: edit.change_count is null and edit.diff_exact is false. With details:true, the existing canonical diff summary is computed and all existing per-op facts are returned; explicit details may be larger and are not silently dropped at a fixed receipt ceiling. Existing operation/payload and workbook host bounds remain.

`editXlsx(originalBytes, requestJson, recalcOptionsJson?)` returns the same typed-byte result with separate metadataJson and free() lifetime. Request fields are existing canonical `ops`, `recalculate: "auto" | "never" | "always"` (auto default), optional `details` and `formula_parse_policy`. No resource IDs or caller revisions are needed. Edits are atomic; failed edits and operational calculation failures return no result bytes. Never and presentation-only Auto avoid evaluator construction; their cache state stays unknown. XML loading and necessary formula translation/validation still occur. Selected calculation uses the chosen-profile source helper on the edited rich document; original-package preservation is not promised across editing. Existing source recalc JSON options/adapter ceilings apply. Native entrypoint is `agent_spreadsheet::stateless_edit::edit_xlsx_bytes` with full upstream options. See the plugin stateless-edits reference for canonical request examples and compact receipt fields.

## Limits

Limits are checked before workbook bytes are copied into WASM or parsed and before parameter JSON is parsed:

- 64 MiB per workbook
- 1 MiB per canonical parameter document
- 16 concurrent sessions
- 256 MiB aggregate source workbook bytes

## Development

```bash
cargo test -p agent-spreadsheet-wasm
cargo check -p agent-spreadsheet-wasm --target wasm32-unknown-unknown
cargo test -p agent-spreadsheet-wasm --target wasm32-unknown-unknown --test wasm_node
cargo test -p agent-spreadsheet-wasm --target wasm32-unknown-unknown --test wasm_write_kinds
cargo test -p agent-spreadsheet-wasm --target wasm32-unknown-unknown --test wasm_browser
cargo clippy -p agent-spreadsheet-wasm --all-targets -- -D warnings
```

The browser test requires a configured WebDriver/browser environment. The Node test uses `wasm-bindgen-test-runner`.

## Stateless source inspection and engine navigation

Development source bindings add `inspectXlsx(inputBytes, requestJson)` and `traceXlsx(inputBytes, requestJson)`. Both return `ReadXlsxResult` with `metadataJson` and `free()`; no workbook bytes are exported. The native `SessionApi::inspect_xlsx/trace_xlsx` returns typed ReadReport/ReadError. Strict unknown fields fail with plain JS code/message errors.

Inspection reports native source values and separately labeled unverified formula caches. Trace uses the native Workbook importer and engine reports without evaluation or a source-cache overlay; NeverEvaluated/null is distinct from source blankness. Default engine max_depth0 materializes root links/direct-neighbor boundary snapshots with Elided/unexplored facts intact. Literal larger limits, details, fingerprint-bound coordinate/materialized-node pages and reroot requests are supported. Native table/external import gaps remain explicit; no whole-graph completeness certification is made. The plugin's references/stateless-inspection.md documents requests, paging, costs and lifetime. These bindings require an explicitly selected matching development runtime; registry0.16 remains unchanged.
