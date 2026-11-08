---
name: spreadsheet
description: Recalculate XLSX formula caches, apply stateless batch edits with optional recalculation and diff receipts, inspect source values/caches, and trace formula precedents/dependents using the pinned Formualizer 0.14 / agent-spreadsheet-wasm 0.17.0 runtime.
---

This skill uses the pinned registry `agent-spreadsheet-wasm@0.17.0` package (backed by Formualizer 0.14.0) to provide stateless, in-process `.xlsx` recalculation, batch editing, source inspection, and formula dependency tracing. Formula engine values are never filled from unverified source caches, and neither read command recalculates.

### Core 4-Verb Stateless Workflow (Preferred)

1. **Recalculate original `.xlsx` formula caches (`source-recalculate`)**:
   ```sh
   node "${CLAUDE_SKILL_DIR}/scripts/run.mjs" source-recalculate /path/input.xlsx --check
   node "${CLAUDE_SKILL_DIR}/scripts/run.mjs" source-recalculate /path/input.xlsx --output /path/recalculated.xlsx
   node "${CLAUDE_SKILL_DIR}/scripts/run.mjs" source-recalculate /path/input.xlsx --options /path/options.json --output /path/recalculated.xlsx
   ```
2. **Batch edit cells/formulas + recalculate + diff (`edit`)**:
   ```sh
   node "${CLAUDE_SKILL_DIR}/scripts/run.mjs" edit /path/input.xlsx --operations /path/ops.json --output /path/edited.xlsx
   ```
   *(Set `"details": true` in `ops.json` to receive the exact cell diff summary alongside the recalculation receipt. See `references/stateless-edits.md`.)*
3. **Inspect source cells, formulas, and stored formula caches (`inspect`)**:
   ```sh
   node "${CLAUDE_SKILL_DIR}/scripts/run.mjs" inspect /path/input.xlsx /path/inspect-request.json
   ```
   *(See `references/stateless-inspection.md` for finite cells/range selection and pagination.)*
4. **Trace formula precedents or dependents (`trace`)**:
   ```sh
   node "${CLAUDE_SKILL_DIR}/scripts/run.mjs" trace /path/input.xlsx /path/trace-request.json
   ```

Use the user's known sheet/range directly. For an unknown location, use targeted canonical discovery and metadata as needed. Do not require broad workbook exploration before a simple edit or read. Reading stored values does not authorize recalculation. Explain calculation state as returned by the runtime.

Run scripts from this skill's folder. `${CLAUDE_SKILL_DIR}` in the examples is a placeholder replaced in skill text by Code/Cowork, not a guaranteed shell environment variable. In chat, locate this copied SKILL.md and use its actual parent directory, then run `scripts/run.mjs` relative to it. All required source, lockfile and installed runtime assets belong inside this folder; no sibling plugin files are needed.

First check `node --version` and `npm --version`. Requires Node >=20 and npm. If Node is absent, report the host prerequisite. Do not fall back to an unrelated `asp` executable on PATH.

```sh
node "${CLAUDE_SKILL_DIR}/scripts/run.mjs" doctor
```

Doctor never installs anything or contacts a registry. If dependencies are missing and installation is permitted by the user's host, explicitly run the reproducible setup in this skill folder:

```sh
cd "${CLAUDE_SKILL_DIR}"
npm ci --ignore-scripts
node scripts/run.mjs doctor
```

Keep install/network protections enabled. If setup is blocked, report that exact prerequisite; do not substitute a runtime or change policy. The sole dependency is exactly agent-spreadsheet-wasm 0.17.0, with no transitive dependencies. npm verifies its archive against package-lock.json integrity during setup.

Canonical discovery (descriptions, availability and costs) comes unchanged from the loaded runtime:

```sh
node "${CLAUDE_SKILL_DIR}/scripts/run.mjs" discover read_cells
```

Omit the operation for the full canonical catalog only when useful. Discovery currently does not include input JSON schemas. Pass canonical parameters unchanged; the runtime validates them. A bounded read request JSON file is:

```json
{"operation":"read_cells","params":{"sheet_name":"Sheet1","selection":{"kind":"range","ranges":["A1:B1"]},"format":"dense"}}
```

```sh
node "${CLAUDE_SKILL_DIR}/scripts/run.mjs" run /path/input.xlsx /path/request.json
node "${CLAUDE_SKILL_DIR}/scripts/run.mjs" run /path/input.xlsx /path/request.json /path/output.xlsx
```

Each canonical run invocation opens a fresh session, executes one operation, optionally exports the workbook, and disposes the session. Without an output argument the source is never written; mutations then disappear at process exit. An explicit output argument writes that destination and overwrites an existing file, including the source if the user explicitly chose it. Parent directories must already exist. Export uses current runtime serialization and is not an original-byte preservation guarantee. Errors are printed on stderr with a nonzero exit code; canonical error envelopes are retained. No persisted sessions/cursors across invocations. Operations such as write require expected_revision from the same resident session and cannot be driven across this one-operation command interface. The canonical run command does not compose revision-sensitive editing; use the separate stateless edit surface with a matching development runtime. Do not invent a revision or silently replace one. Session-bound artifact handles are not downloadable through this minimal wrapper. Workbook export is the supported artifact route here.

For file-in/batch-edit/optional-recalc/file-out requests, use the stateless `edit INPUT.xlsx --operations REQUEST.json --output OUTPUT.xlsx` surface described in `references/stateless-edits.md`. Do not ask the user to manage session IDs, revisions or preview/stage cycles for this surface. Auto is the default batch heuristic; skipped calculation leaves cache freshness unknown. Rich editing is not an original-package preservation guarantee.

For explicitly requested cache-only recalculation, follow `references/source-runtime.md`. The `source-recalculate INPUT.xlsx [--options OPTIONS.json] [--output OUTPUT.xlsx | --check]` command uses original bytes before session creation. This path returns diagnostics and a receipt, and writes only an explicit output destination. File-facing defaults use the workbook profile; explicit strict and requireComplete remain available. Check coverage/completed separately from bytesUnchanged and legacy current (byte equality only). A typed deferred result may retain valid input/edited bytes without fresh caches; report that deferral explicitly. It does not change the cached-read probe semantics or authorize calculation during a read.

For actual-host transport testing, follow `references/host-probe.md`. The generator `scripts/synthetic.mjs` makes the tiny fixture from XML text without binary assets. The focused tests in `test/transport.test.mjs` run against the installed pinned WASM: `npm test`.

Return a short receipt: the selected values, reported calculation state, and the output path if written. In chat/Cowork, attach or link the exported file using that host's normal artifact mechanism. A local file path alone does not prove delivery to the user. Do not claim recalculation, engine upgrades, full fidelity or successful host testing from this transport probe.
