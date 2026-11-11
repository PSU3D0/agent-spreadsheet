# Stateless batch edits

The browser-facing operation is file-in → batch edits → optional recalculation → file-out. Each invocation opens its own document and returns bytes; no persistent session ID, caller revision, preview or stage cycle is needed.

The bundled stateless engine package includes `editXlsx` out of the box.

```sh
node scripts/xlsx.mjs edit INPUT.xlsx --operations REQUEST.json --output OUTPUT.xlsx
node scripts/xlsx.mjs edit INPUT.xlsx --operations-json '{"ops":[{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":30}}}],"recalculate":"auto"}' --output OUTPUT.xlsx
node scripts/xlsx.mjs edit INPUT.xlsx --operations REQUEST.json --output OUTPUT.xlsx --recalc-options RECALC_OPTIONS.json
```

The output path is explicit and follows ordinary overwrite intent, including the input path if explicitly chosen. Calculation/edit/option failures return no successful artifact and do not overwrite an existing destination. Filesystem write failures have ordinary filesystem behavior; no CAS or atomic filesystem publication is promised.

Request example:

```json
{
  "ops": [
    {"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":30}}}
  ],
  "recalculate": "auto"
}
```

Existing canonical WriteOp vocabulary carries cell types, transforms, formulas, styles, layout, rules, names, imports and structural edits. Existing per-operation fields, validation and batch limits remain in force; no smaller operation allowlist is imposed. For example, overwriting an existing formula with set_cells requires its existing `overwrite_formulas:true` field:

```json
{
  "ops": [
    {"kind":"set_cells","sheet_name":"Sheet1","overwrite_formulas":true,
     "cells":{"B1":{"kind":"formula","formula":"A1*3"}}}
  ],
  "recalculate": "always",
  "details": true
}
```

Top-level fields are `ops`, optional `recalculate` (auto default), optional `details` (false default), and optional existing `formula_parse_policy` (warn default; fail/warn/off). Unknown fields—including resource_id, expected_revision, stage and preview plumbing—are rejected. All edits are atomic at the byte-operation level: a failed batch or requested strict recalculation returns an error, not a partially edited result.

Modes:
- `never`: edit/export without constructing an evaluator or running calculation. Existing caches are unvalidated; rich serialization may retain, change or remove them.
- `auto`: reuse the existing canonical operation-category classifier. Presentation/layout/rules-only batches (styles, sizing, freeze panes, zoom, gridlines, print/page settings, data validation and conditional-format settings) skip calculation. Value/formula/name/structural/import/other invalidating batches select it. This is a batch heuristic, not proof that incoming caches were current.
- `always`: run selected-profile recalculation after successful editing, including for style/layout-only batches.

No-recalc still loads XML into a rich document, and structural/formula editing may parse/translate formulas when required. The compiled WASM also includes engine code. Skipping evaluation does not mean zero XML parsing, zero formula parsing or zero binary loading.

When selected, calculation calls the same high-level source byte helper on **edited document bytes**. Rich editing may rewrite/drop unsupported original content. Strict package preservation applies to subsequent recalculation relative to that edited package, not to the original uploaded file. Upstream refusals are propagated without falling back to resident calculation. Native callers retain full upstream EvalConfig/cancellation/limits; the JS `--recalc-options` JSON accepts exactly the already documented source-binding options and rejects unavailable/unknown fields even in never mode. The output ceiling remains capped before recalculation construction by the inherited adapter guard and also applies to skipped/deferred edited output. File-facing defaults use workbook; explicit strict preserves legacy guards. Only typed CalculationDeferred in workbook mode with requireComplete:false preserves an edited artifact and reports deferred coverage; other failures remain errors. See source-runtime.md for the shared portable calculation schema.

Receipt stdout includes runtime provenance, input/output/result hashes, bytes_unchanged (byte equality only) and metadata:
- `edit`: ops_applied, op_kinds, canonical change_count/diff_exact, calculation requested/performed/reason/recommendation/cache_state.
- Skipped calculation reports `cache_state:"unknown"`; no incoming/current-cache guarantee is invented. Invalidating categories recommend recalculation; preserving categories leave need unknown.
- `recalculation`: present when attempted; shared effective settings, coverage/operation diagnostics and exact summary/error/sample/clock/cache-patch metadata. A typed deferred result has null unknown counts and no cache freshness claim. `attempted`, `completed`, and `artifact_available` are independent; `performed` stays false on deferral.
- Default requests skip the full package diff: `change_count:null` means not computed and `diff_exact:false` makes that uncertainty explicit. Editing and optional calculation still run normally. This avoids the diff's extra ZIP/XML reads and materialized changes; rich document import/export and operation validation still occur.
- Explicit `details:true` computes the existing canonical diff summary and returns all existing per-op typed facts. `change_count` then contains the computed count and `diff_exact` reports its precision. Details may be larger; there is no fixed 64KiB omission policy. Existing operation/payload and workbook host limits still apply. No additional setting or retrieval step is required.

Native API:
`agent_spreadsheet::stateless_edit::edit_xlsx_bytes(input, StatelessEditRequest, XlsxRecalculateOptions)`.
The result exposes bytes() and an EditedArtifact (Document, upstream Recalculated result, or typed Deferred edited bytes), plus a compact receipt. The module uses the current recalc-formualizer feature because the existing canonical_write module is coupled to resident types; no unrelated feature-gating cleanup was added.

Direct byte API:
`SessionApi::edit_xlsx(input, request_json, recalc_options_json?)`.
Generated JS:
`editXlsx(Uint8Array, requestJson, recalcOptionsJson?)`.
Returned typed bytes and metadataJson are separate; free() must be called in finally. The plugin helper does this on success and decoding/write failure. Plain error payloads preserve code and reason.

Two independent CLI invocations may reopen the first output and apply another batch without any shared handles. No reopen-latency or hyperfast-performance claim is made. Existing resident APIs remain available for callers that deliberately use them.
