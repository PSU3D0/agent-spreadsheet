# User-run host probe

Run separately in Claude Code, Cowork and claude.ai chat, using only a synthetic workbook. Actual-host results are pending; local Node tests do not establish host support.

1. Load this plugin locally in Code with `claude --plugin-dir /absolute/path/to/plugins/formualizer`. For Cowork/chat, upload the plugin ZIP to your own account following the official plugin docs. The manifest must be at `.claude-plugin/plugin.json` inside the archive root (or one enclosing plugin folder). Alternatively test the standalone `spreadsheet/` skill ZIP; it must contain `spreadsheet/SKILL.md`.
2. Ask Claude to locate the spreadsheet skill and report Node/npm versions. If Node >=20 is unavailable, record a blocked runtime prerequisite.
3. With normal install permission/network access, ask it to run `npm ci --ignore-scripts` in the copied skill folder, then `node scripts/run.mjs doctor`. Do not disable host protections. A prepared copy may include its skill-local node_modules; scripts never download dependencies automatically. Do not package unrelated caches or credentials.
4. Ask it to run `node scripts/run.mjs discover read_cells`, then `node scripts/run.mjs probe /absolute/writable/path/NEW-probe-directory`. Choose the host's writable artifact location and a directory that does not yet exist. This generates input.xlsx, request.json, export.xlsx and receipt.json. No private workbook is involved.
5. Ask it to attach export.xlsx and receipt.json through that host's artifact UI. Download export.xlsx and confirm it opens with Sheet1 A1=21 and B1 formula A1*2 with supplied cache 42. The probe has already reopened the export through WASM and checked the bounded read. B1's cached value is not evidence of calculation.
6. Record host/app version, date, plugin version 0.1.0-alpha.1, Node/npm/package versions, doctor output, exact commands, install/permission limitations, bounded read and calculation state, export/reopen success, and whether the user could actually download/open the artifact. Save evidence outside the plugin source.

Suggested prompt:

> Use the Formualizer spreadsheet prototype on synthetic data only. Follow references/host-probe.md, report prerequisites first, and run the bounded read/export probe in a new writable artifact directory. Return export.xlsx and receipt.json as downloadable artifacts. Report actual calculation state and any blocked step without changing host protections.

No credentials, hosted spreadsheet service, local MCP server, global installation or settings edits are required. Installation may require registry access; spreadsheet bytes remain in the host execution environment. Chat execution means a sandbox with Node/WASM support, not the browser DOM.
