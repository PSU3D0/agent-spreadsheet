---
name: spreadsheet
description: Recalculate XLSX caches, batch edit workbooks, inspect source cells and caches, and trace formula dependencies with bundled Formualizer 0.14.0.
---

Requires Node >=20. The engine is bundled inside this skill; no npm install, network access, credentials, or local server is required. Locate this copied SKILL.md and use its parent directory as the skill directory. All required runtime files are inside it.

Run the integrity and runtime check:

```sh
node "${CLAUDE_SKILL_DIR}/scripts/xlsx.mjs" doctor
```

Use these four file commands:

```sh
node "${CLAUDE_SKILL_DIR}/scripts/xlsx.mjs" source-recalculate /path/input.xlsx --check
node "${CLAUDE_SKILL_DIR}/scripts/xlsx.mjs" source-recalculate /path/input.xlsx --output /path/recalculated.xlsx
node "${CLAUDE_SKILL_DIR}/scripts/xlsx.mjs" edit /path/input.xlsx --operations /path/ops.json --output /path/edited.xlsx
node "${CLAUDE_SKILL_DIR}/scripts/xlsx.mjs" inspect /path/input.xlsx /path/inspect-request.json
node "${CLAUDE_SKILL_DIR}/scripts/xlsx.mjs" trace /path/input.xlsx /path/trace-request.json
```

Use the user's known sheet/range directly. Inspect and trace do not recalculate; stored formula caches are unverified. Reading does not authorize recalculation. See references/stateless-inspection.md for selection, paging and trace budgets; references/stateless-edits.md for batch operations and optional recalculation; references/source-runtime.md for calculation options and receipts.

Only an explicit output destination writes a workbook. It overwrites an existing destination, including the input if explicitly selected; parent directories must exist. Rich edits serialize the workbook and do not guarantee original-package preservation. Cache-only recalculation uses original source bytes. Check coverage/completed separately from bytesUnchanged and legacy current (both mean byte equality only); report deferred calculation explicitly.

The readable loader verifies the bundled Brotli file against engine/PROVENANCE.json, decompresses it locally with Node zlib, verifies the raw WASM hash and instantiates it. The skill and local stdio paths send no network requests or telemetry. The compiled engine requires plugin-directory review.

Return selected values, reported calculation state, and the output artifact if written. Attach or link it using the host's artifact mechanism. Local checks do not establish downloadable artifact delivery in another host. For synthetic host checks see references/host-probe.md.
