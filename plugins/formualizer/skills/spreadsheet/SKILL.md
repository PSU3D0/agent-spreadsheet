---
name: spreadsheet
description: Fast Excel (.xlsx) formula recalculation, targeted cell reads, batch edits, and dependency tracing. Use to explain unexpected totals, trace precedents or dependents, test assumptions, fix stale results, and calculate workbooks created or edited with other tools.
---

Route tasks to the matching `formualizer:spreadsheet` operation:

- **Explain formulas, drivers, or unexpected totals (read-only):** run `trace` (`--roots ... --direction precedents --max-depth 2`) to map formula dependencies and `inspect` (`--cells ...` or `--range ...`) to read formulas and stored cached values. Do **not** run `source-recalculate` on explanation or read-only requests unless the user asks to verify or repair stale caches, or stored caches are missing.
- **Test assumptions or edit cells/formulas:** run `edit` to apply atomic `set_cells` changes and recalculate in one step, then `inspect` affected output cells and verify the receipt.
- **Calculate a final deliverable from other tools:** when creating or modifying a formula-bearing `.xlsx` with another library, run `source-recalculate --output /path/final.xlsx` (or `--check` for read-only verification) after saving final changes and before returning the file.

`inspect` and `trace` never evaluate formulas or modify files. Stored formula caches carry `freshness: "unverified"`. Reading or explaining a workbook does not authorize recalculation.

## Run locally

Requires Node >=20. The engine is bundled; no install, network access, credentials, or local server is required. Locate this `SKILL.md` and set `CLAUDE_SKILL_DIR` to its parent directory.

Check the bundled engine:

```sh
node "${CLAUDE_SKILL_DIR}/scripts/xlsx.mjs" doctor
```

### 1. Inspect cells or ranges (read-only)

Pass `--cells` or `--range` directly (addresses must be sheet-qualified, such as `Sheet1!A1` or `'My Sheet'!A1:D20`):

```sh
node "${CLAUDE_SKILL_DIR}/scripts/xlsx.mjs" inspect /path/input.xlsx --cells "Sheet1!B1" "Sheet1!G12"
node "${CLAUDE_SKILL_DIR}/scripts/xlsx.mjs" inspect /path/input.xlsx --range "Sheet1!A1:M20" --limit 100
```

Or pass canonical JSON via `--json '<JSON>'` or a `REQUEST.json` file. Top-level fields are singular `selection` and optional `page` only (never `selections` or `sheets`):

```json
{"selection":{"kind":"cells","addresses":["Sheet1!A1","Sheet1!B1"]}}
{"selection":{"kind":"range","address":"Sheet1!A1:D20"},"page":{"limit":50}}
```

### 2. Trace precedents or dependents (read-only)

Trace upstream inputs (`precedents`) or downstream formulas (`dependents`):

```sh
node "${CLAUDE_SKILL_DIR}/scripts/xlsx.mjs" trace /path/input.xlsx --roots "Sheet1!G12" --direction precedents --max-depth 2
node "${CLAUDE_SKILL_DIR}/scripts/xlsx.mjs" trace /path/input.xlsx --roots "Assumptions!B9" --direction dependents --max-depth 2
```

Or pass canonical JSON via `--json '<JSON>'` or `REQUEST.json` (including verbatim `next_trace_request` or `next_request` cursors). Top-level fields are `roots` plus optional `direction`, `limits`, `details`, and `page`:

```json
{"roots":["Sheet1!G12"],"direction":"precedents","limits":{"max_depth":2}}
```

Traces are bounded (`max_depth` defaults to `0` if omitted) and do not guarantee the entire dependency graph. Follow returned `next_trace_request` or `next_inspect_request` payloads for deeper hops. See [stateless-inspection.md](references/stateless-inspection.md).

### 3. Batch edit assumptions or formulas

Apply atomic changes and recalculate in one call via `--operations-json` or `--operations /path/ops.json`:

```sh
node "${CLAUDE_SKILL_DIR}/scripts/xlsx.mjs" edit /path/input.xlsx \
  --operations-json '{"ops":[{"kind":"set_cells","sheet_name":"Sheet1","cells":{"A1":{"kind":"value","value":30}}}],"recalculate":"auto"}' \
  --output /path/edited.xlsx
```

To replace an existing formula in `set_cells`, include `"overwrite_formulas": true` and `{"kind":"formula","formula":"A1*3"}` (without leading `=`). Set `"details": true` on the request for an exact cell diff. Rich editing can rewrite the workbook package and does not guarantee original-package preservation; see [stateless-edits.md](references/stateless-edits.md).

### 4. Recalculate original workbook bytes

Check cached formula results without writing, or write recalculated caches while preserving non-cache package bytes:

```sh
node "${CLAUDE_SKILL_DIR}/scripts/xlsx.mjs" source-recalculate /path/input.xlsx --check
node "${CLAUDE_SKILL_DIR}/scripts/xlsx.mjs" source-recalculate /path/input.xlsx --output /path/recalculated.xlsx
```

See [source-runtime.md](references/source-runtime.md) for `--options` / `--options-json` calculation settings and receipt fields.

## Check and deliver

Only an explicit `--output` argument writes a workbook. It overwrites an existing destination, including the input if explicitly selected; parent directories must exist.

Read receipt `coverage`, `completed`, and error counts before reporting calculated results. `bytesUnchanged` and legacy `current` mean byte equality only. Report partial or deferred calculation explicitly; an available artifact alone does not establish completed calculation or fresh caches.

Verify comparative narrative claims (such as minimums, troughs, percentage shares, or counterfactuals) against all relevant periods and cells before stating them. Attach or link any requested saved workbook using the host's artifact mechanism and verify that the output file exists.
