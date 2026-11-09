# Formualizer Spreadsheet Plugin

Formualizer 0.17.0 supplies a portable spreadsheet skill, a narrow 4-verb local `stdio` MCP server, and a stateless Cloudflare Edge MCP/OpenAPI worker backed by the bundled stateless WASM engine (Formualizer 0.14.0). It provides stateless original-source `.xlsx` recalculation (`source-recalculate`), atomic batch editing with optional recalculation and diff receipts (`edit`), source cell/cache inspection (`inspect`), and engine-backed precedent/dependent tracing (`trace`).

Requires Node >=20 and a writable execution environment. The engine is bundled; no dependency installation or network is required. To install in Claude Code from the marketplace repository:

```text
/plugin install formualizer --marketplace PSU3D0/formualizer-plugin
```

From this directory:

```sh
cd skills/spreadsheet
node scripts/xlsx.mjs doctor
node --test test/*.test.mjs
```

Doctor verifies the bundled compressed and raw WASM hashes against PROVENANCE.json and instantiates the engine locally. The compiled engine is held for manual directory review.

For use on a workbook, see [the skill](skills/spreadsheet/SKILL.md). Inspect and trace read source values and unverified caches without recalculation. Only explicit output destinations write files. Rich edits serialize the workbook; cache-only recalculation uses original source bytes. Actual-host execution and artifact delivery still require separate evidence; see [host checks](skills/spreadsheet/references/host-probe.md).

## Privacy & Data Handling

Runtime operations (`source-recalculate`, `edit`, `inspect`, `trace`, `doctor`) and the local stdio MCP server execute in the host environment using the bundled engine. The Brotli WASM is decompressed locally with Node zlib. Skill and stdio paths send no network requests or telemetry and require no credentials. Host platform data handling still applies. The optional edge worker runs in its deployment environment and retains its explicit telemetry callback.

Layout follows [official plugin guidance](https://claude.com/docs/plugins/build.md) and [skill script guidance](https://claude.com/docs/skills/how-to.md#add-scripts).

Stateless inspection and dependency navigation: use `inspect INPUT.xlsx REQUEST.json` or `trace INPUT.xlsx REQUEST.json`. Source caches remain explicitly unverified; engine values/currentness are separate and neither read command evaluates formulas. See [stateless inspection](skills/spreadsheet/references/stateless-inspection.md) for one-hop budgets, rerooting, pagination, and lifetime details.
