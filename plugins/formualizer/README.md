# Formualizer Spreadsheet Plugin

Formualizer 0.17.0 supplies a portable spreadsheet skill, a narrow 4-verb local `stdio` MCP server, and a stateless Cloudflare Edge MCP/OpenAPI worker backed by the pinned `agent-spreadsheet-wasm@0.17.0` (Formualizer 0.14.0) runtime. It provides stateless original-source `.xlsx` recalculation (`source-recalculate`), atomic batch editing with optional recalculation and diff receipts (`edit`), source cell/cache inspection (`inspect`), and engine-backed precedent/dependent tracing (`trace`).

Requires Node >=20, npm, a writable execution environment and either registry access for explicit setup or a prepared skill-local dependency installation. To install in Claude Code from the marketplace repository:

```text
/plugin install formualizer --marketplace PSU3D0/formualizer-plugin
```

From this directory:

```sh
cd skills/spreadsheet
npm ci --ignore-scripts
node scripts/run.mjs doctor
node scripts/run.mjs discover read_cells
node scripts/run.mjs probe /absolute/writable/path/new-formualizer-probe
npm test
```

The only dependency is exactly agent-spreadsheet-wasm 0.17.0; it has no transitive dependencies. Its lockfile archive integrity is:

```text
sha512-LpmEt1GNA91zvalLKJ4/J7VKe+gMpneBsoe8ZukQKwCSulIh7DAoXuiVwrEjCPoVaTRwcwlZa8407zFsSoYtag==
```

The wrapper needs Node 20+ for WebCrypto-backed session creation: unflagged Node 18.19.1 failed the real session probe, while Node 20.20.2, 22.22.1, 24.22.0 and 26.5.0 passed. This is a runtime prerequisite, not a Lite/Full distinction.

The dependency version differs from the plugin version. npm ci verifies the archive integrity; doctor checks the installed package version, lock identity, required bindings and actual WASM startup. Doctor does not download or install packages, and does not independently hash every installed file. The source-only distribution includes the exact lockfile and requires explicit setup; to prepare an offline copy, run that setup before copying the entire skill folder including node_modules. All runtime assets resolve within the skill; neither the repository's SDK nor an asp command on PATH is used.

For matching source-generated JS/WASM, the explicit local loader flag, original-source check/output commands and local test instructions, see [development source-runtime proof](skills/spreadsheet/references/source-runtime.md). Default 0.16.0 reports source recalculation unavailable and never substitutes resident serialization. The [stateless batch-edit surface](skills/spreadsheet/references/stateless-edits.md) adds file-in/batch-edit/optional-recalc/file-out through the same explicit development loader; it also remains unavailable in the default pin.

For use on a workbook, see [the skill](skills/spreadsheet/SKILL.md). Supply a canonical request file and optionally an export destination. No output argument means no source write. A supplied destination follows ordinary filesystem overwrite behavior, including the source path if explicitly selected. Export is the current runtime's serialization, not a byte-preservation promise. Each canonical run call opens a fresh session and closes it; this prototype supports workbook artifacts, not persistent session handles or image-artifact downloads. Revision-sensitive operations such as write need same-session expected_revision and cannot be composed across these one-operation calls; use the separate stateless edit surface when available. Canonical discovery can list operations beyond this wrapper’s practical single-call surface.

The probe generates a tiny workbook from XML source, reads two cells, exports, reopens and reads again. A1=21 and B1's supplied cache=42 demonstrate transport only. This is not upgraded engine or source-preservation evidence. Local Node tests are possible here; successful Claude Code, Cowork and chat execution and downloadable artifacts still require separate actual-host evidence. Follow [host probe instructions](skills/spreadsheet/references/host-probe.md). Plugin scripts are supported components on those surfaces, but interpreter availability, setup permissions and artifact delivery remain host prerequisites.

Runtime operations send no spreadsheet data to a hosted service and require no credentials. Workbooks stay in the execution environment chosen by the Claude host; normal Claude execution/data handling still applies. Explicit npm setup contacts the npm registry. There are no hooks, top-level executables, auto-installers or local MCP requirements. This prototype has not been published or submitted.

Layout follows [official plugin guidance](https://claude.com/docs/plugins/build.md) and [skill script guidance](https://claude.com/docs/skills/how-to.md#add-scripts). The parent session will run Claude's manifest validator and a local plugin session separately.

Opt-in development inspection/navigation: use `inspect INPUT.xlsx REQUEST.json` or `trace INPUT.xlsx REQUEST.json` with matching local bindings. Source caches remain explicitly unverified; engine values/currentness are separate and neither command evaluates. See [stateless inspection](skills/spreadsheet/references/stateless-inspection.md) for one-hop budgets, rerooting, pagination, lifetime and native import coverage gaps.
