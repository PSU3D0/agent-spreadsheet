---
name: spreadsheet
description: Fast Excel (.xlsx) formula recalculation, targeted cell reads, batch edits, and dependency tracing. Use to explain unexpected totals, test assumptions, fix stale results, and calculate workbooks created or edited with other tools.
---

Use `formualizer:spreadsheet` for these tasks:

- **Inspect selected cells:** read values, formulas and stored results from the user's known sheet/range. Use focused selections and paginate larger results.
- **Explain unexpected totals:** trace precedents to find what feeds a formula, or dependents to see what an assumption affects. Follow returned requests for more detail; traces are bounded and do not guarantee the entire dependency graph.
- **Batch edit assumptions or formulas:** collect related changes in one operations request, then inspect the affected outputs and calculation receipt.
- **Calculate the final deliverable:** when the user requests creating or editing a formula-bearing XLSX, use Formualizer to calculate the final saved workbook before returning it. This complements other tools used to create or style the file; run it after those tools save their final changes.

Inspect and trace do not evaluate formulas. Stored formula caches are unverified. Reading alone does not authorize recalculation.

## Run locally

Requires Node >=20. The engine is bundled; no install, network access, credentials or local server is required. Locate this copied `SKILL.md` and use its parent directory as `CLAUDE_SKILL_DIR`; all required runtime files are inside it.

Check the bundled engine:

```sh
node "${CLAUDE_SKILL_DIR}/scripts/xlsx.mjs" doctor
```

Inspect selected cells or trace dependencies using a request JSON file:

```sh
node "${CLAUDE_SKILL_DIR}/scripts/xlsx.mjs" inspect /path/input.xlsx /path/inspect-request.json
node "${CLAUDE_SKILL_DIR}/scripts/xlsx.mjs" trace /path/input.xlsx /path/trace-request.json
```

See [stateless-inspection.md](references/stateless-inspection.md) for cell/range selections, precedents/dependents, paging and trace budgets. Use the user's known sheet/range directly and follow returned paging or trace requests as needed.

Apply a batch of edits:

```sh
node "${CLAUDE_SKILL_DIR}/scripts/xlsx.mjs" edit /path/input.xlsx --operations /path/ops.json --output /path/edited.xlsx
```

See [stateless-edits.md](references/stateless-edits.md) for operations and recalculation modes. Edits can rewrite the workbook, including content unsupported by the editor; they do not guarantee original-package preservation.

Recalculate a saved workbook, either checking without writing or producing the final artifact:

```sh
node "${CLAUDE_SKILL_DIR}/scripts/xlsx.mjs" source-recalculate /path/input.xlsx --check
node "${CLAUDE_SKILL_DIR}/scripts/xlsx.mjs" source-recalculate /path/input.xlsx --output /path/recalculated.xlsx
```

Original-source recalculation preserves non-cache bytes. See [source-runtime.md](references/source-runtime.md) for calculation options and receipt fields.

## Check and deliver

Only an explicit output argument writes a workbook. It overwrites an existing destination, including the input if explicitly selected; parent directories must exist.

Read receipt coverage and `completed` before reporting calculated results. `bytesUnchanged` and legacy `current` mean byte equality only. Report partial or deferred calculation explicitly; an available artifact alone does not establish completed calculation or fresh caches.

Return the selected values, reported calculation state and output artifact if written. Attach or link the saved workbook using the host's artifact mechanism and verify that the output file exists.
