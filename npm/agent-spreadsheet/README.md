# agent-spreadsheet

[![npm](https://img.shields.io/npm/v/agent-spreadsheet.svg)](https://www.npmjs.com/package/agent-spreadsheet)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](https://github.com/PSU3D0/agent-spreadsheet/blob/main/LICENSE)

**`agent-spreadsheet` is the npm distribution of the agent-spreadsheet CLI — the tool interaction surface for agent-based spreadsheet work.**

This package installs a prebuilt native binary and exposes:
- `asp` — primary command
- `agent-spreadsheet` — compatibility alias

No Rust toolchain required.

---

## Why this package exists

Use this package when you want spreadsheet automation for agents, scripts, CI, or local tooling without standing up an MCP server.

It is built for:
- exact workbook reads
- safe file-based edits
- deterministic JSON contracts
- verification and grouped diffs
- preview-first workflow helpers
- schema/example discovery for agent prompts and tool callers

If your use case is **one-shot command execution**, this is the fastest way in.

---

## Install

```bash
npm i -g agent-spreadsheet
asp --help
```

Installs both commands:

```bash
asp --version
agent-spreadsheet --version
```

---

## What you get

### Read and inspect
- list sheets
- detect regions and table-like structure
- read exact ranges
- inspect targeted cells
- inspect layout metadata
- export bounded ranges to csv or grid json

### Analyze and reason
- find values or labels
- find formulas
- trace precedents and dependents
- profile tables and sheets
- scan volatile formulas
- preview structural reference impact without mutation

### Mutate safely
- direct cell edits
- import grid/csv data
- footer-aware append
- template row and row-band cloning
- formula-only replace
- named range CRUD
- stateless batch operations for transform/style/structure/layout/rules

### Prove outcomes
- `verify proof` for target deltas + error provenance
- `verify diff` for grouped, review-friendly workbook diffs

### Scale up when needed
- event-sourced `session` commands
- SheetPort manifest lifecycle and execution

---

## Quickstart

## Orient the workbook

```bash
asp read sheets data.xlsx
asp read overview data.xlsx "Model"
asp read table data.xlsx --sheet "Model"
asp read names data.xlsx
asp read page data.xlsx Sheet1 --format compact --page-size 200
```

## Inspect exact cells and layout

```bash
asp read values data.xlsx Model A1:C20
asp read cells data.xlsx Model B2 D8:F10
asp read layout data.xlsx Model --range A1:H30 --render both
asp read export data.xlsx Model A1:H30 --format csv --output model.csv
```

## Safe stateless edit workflow

```bash
asp workbook copy data.xlsx /tmp/draft.xlsx
asp write cells /tmp/draft.xlsx Inputs "B2=500" "C2==B2*1.1"
asp workbook recalculate /tmp/draft.xlsx
asp verify proof data.xlsx /tmp/draft.xlsx --targets Summary!B2,Summary!B3
asp verify diff data.xlsx /tmp/draft.xlsx --details --limit 50
asp analyze find-value data.xlsx "Net Income" --mode label --label-direction below
```

## Preview-first workflow helpers

```bash
asp write batch transform data.xlsx --ops @ops.json --dry-run
asp write append data.xlsx --sheet Revenue --table-name RevenueTable --from-csv rows.csv --header --dry-run
asp write clone-template-row data.xlsx --sheet Inputs --source-row 8 --after 8 --count 3 --dry-run
asp write clone-row-band data.xlsx --sheet Forecast --source-rows 12:16 --after 16 --repeat 4 --dry-run
```

## Session workflow

```bash
asp session start --base data.xlsx --workspace .
asp session op --session <id> --ops @ops.json --workspace .
asp session apply --session <id> <staged_id> --workspace .
asp session materialize --session <id> --output result.xlsx --workspace .
```

## SheetPort workflow

```bash
asp sheetport manifest candidates model.xlsx
asp sheetport manifest validate manifest.yaml
asp sheetport bind-check model.xlsx manifest.yaml
asp sheetport run model.xlsx manifest.yaml --inputs @inputs.json
```

---

## Command groups

The preferred surface is the nested `asp` command structure:

- `asp read ...`
- `asp analyze ...`
- `asp write ...`
- `asp workbook ...`
- `asp verify ...`
- `asp session ...`
- `asp sheetport ...`

`agent-spreadsheet` remains supported as a compatibility alias.

---

## High-signal commands

### `read`

| Command | Purpose |
| --- | --- |
| `asp read sheets <file>` | List workbook sheets |
| `asp read overview <file> <sheet>` | Detect regions and structure |
| `asp read values <file> <sheet> <range> [range...]` | Read raw values |
| `asp read export <file> <sheet> <range>` | Export to csv or grid json |
| `asp read cells <file> <sheet> <target> [target...]` | Inspect exact cells/ranges |
| `asp read page <file> <sheet> ...` | Deterministic paging with `next_start_row` |
| `asp read table <file> ...` | Structured table/region reads |
| `asp read names <file>` | Named ranges and table/formula items |
| `asp read workbook <file>` | Workbook metadata |
| `asp read layout <file> <sheet>` | Layout-aware rendering |

### `analyze`

| Command | Purpose |
| --- | --- |
| `asp analyze find-value <file> <query>` | Search by value or label |
| `asp analyze find-formula <file> <query>` | Search formulas |
| `asp analyze formula-map <file> <sheet>` | Summarize formulas |
| `asp analyze formula-trace <file> <sheet> <cell> ...` | Trace precedents/dependents |
| `asp analyze scan-volatiles <file>` | Find volatile formulas |
| `asp analyze table-profile <file>` | Profile structured data |
| `asp analyze sheet-statistics <file> <sheet>` | Density/type statistics |
| `asp analyze ref-impact <file> --ops @structure_ops.json` | Preview structural reference impact |

### `write`

| Command | Purpose |
| --- | --- |
| `asp write cells <file> <sheet> ...` | Direct shorthand cell edits |
| `asp write import <file> <sheet> ...` | Import grid/csv data |
| `asp write append ...` | Footer-aware row append |
| `asp write clone-template-row ...` | Clone one template row |
| `asp write clone-row-band ...` | Clone repeated row bands |
| `asp write formulas replace ...` | Formula-only find/replace |
| `asp write name define|update|delete ...` | Named range mutation helpers |
| `asp write batch transform ...` | Stateless transforms |
| `asp write batch style ...` | Stateless style edits |
| `asp write batch formula-pattern ...` | Formula fill/pattern application |
| `asp write batch structure ...` | Structure mutations |
| `asp write batch column-size ...` | Column width operations |
| `asp write batch sheet-layout ...` | Layout operations |
| `asp write batch rules ...` | Data validation / conditional formatting |

### `verify`

| Command | Purpose |
| --- | --- |
| `asp verify proof <baseline> <current>` | Prove target deltas and isolate new/resolved/preexisting errors |
| `asp verify diff <original> <modified>` | Grouped, review-friendly workbook diff |

### `session`

Native 0.16 sessions retain their document and Formualizer evaluator in an automatically started private host, with journal-backed history and request outcomes. Use `--request-id` (or `ASP_REQUEST_ID`) for retry reconciliation and `--expected-revision` for explicit CAS. Normal file commands remain stateless; `materialize` is the explicit session-to-XLSX export boundary. Legacy session migration is not provided.

| Command | Purpose |
| --- | --- |
| `asp session start ...` | Start a session |
| `asp session log ...` | Inspect event history |
| `asp session branches ...` | List branches |
| `asp session switch ...` | Switch branches |
| `asp session checkout ...` | Time-travel to an event |
| `asp session undo` / `redo` | Branch-local history navigation |
| `asp session fork ...` | Branch from a point in history |
| `asp session op ...` | Stage a dry-run operation |
| `asp session apply ...` | Apply a staged change |
| `asp session materialize ...` | Emit a workbook file |

### `sheetport`

| Command | Purpose |
| --- | --- |
| `asp sheetport manifest candidates <file>` | Discover candidate ports |
| `asp sheetport manifest schema` | Print schema |
| `asp sheetport manifest validate <manifest>` | Validate manifest |
| `asp sheetport manifest normalize <manifest>` | Normalize manifest |
| `asp sheetport bind-check <file> <manifest>` | Verify workbook/manifest binding |
| `asp sheetport run <file> <manifest>` | Execute a manifest with JSON inputs |

---

## Output contracts for agents

### JSON first
All commands emit JSON by default.

### Canonical vs compact
Use:

```bash
--shape canonical
--shape compact
```

For `range-values`, shape policy is:
- **Canonical (default/omitted): return `values: [...]` when entries are present; omit `values` when all requested ranges are pruned (for example, invalid ranges).**
- **Compact (single range):** flatten that entry to top-level fields (`range`, payload, optional `next_start_row`).
- **Compact (multiple ranges):** keep `values: [...]` with per-entry `range`.

For other high-traffic commands:
- `read-table` and `sheet-page` compact mode preserves active response branches and continuation fields (`next_offset`, `next_start_row`).
- `formula-trace` compact mode omits per-layer highlights but preserves `layers` and `next_cursor`.

### CLI reference excerpts
- `read page <file> <sheet> --format <full|compact|values_only> [--start-row ROW] [--page-size N]`
- `analyze find-value <file> <query> [--sheet S] [--mode value\|label] [--label-direction right\|below\|any]`
- `write batch transform <file> --ops @ops.json (--dry-run\|--in-place\|--output PATH)`

### Self-describing payloads
Ask the CLI for contracts directly:

```bash
asp schema write batch transform
asp example write batch transform
asp schema session op transform.write_matrix
asp example session op transform.write_matrix
```

### Formula parse policy
Formula-aware commands support:

```bash
--formula-parse-policy fail|warn|off
```

Use `warn` when you want progress plus grouped diagnostics.

Global `--output-format csv` is currently unsupported; use command-specific CSV options such as `read table --table-format csv`.

`write batch formula-pattern` clears cached results for touched formula cells; run `workbook recalculate` to refresh computed values.

---

## Platform support

Prebuilt binaries are downloaded on install for:

| Platform | Architecture | Asset |
| --- | --- | --- |
| Linux | x86_64 | `agent-spreadsheet-linux-x86_64` |
| macOS | x86_64 | `agent-spreadsheet-macos-x86_64` |
| macOS | arm64 | `agent-spreadsheet-macos-aarch64` |

Windows native binaries are not shipped for 0.16.

---

## Environment variables

| Variable | Description |
| --- | --- |
| `AGENT_SPREADSHEET_LOCAL_BINARY` | Use a prebuilt local binary instead of downloading |
| `AGENT_SPREADSHEET_DOWNLOAD_BASE_URL` | Override the release download host |

---

## Troubleshooting

### Binary not found after install

If you see `BINARY_NOT_INSTALLED`, re-run install:

```bash
npm i -g agent-spreadsheet
```

And verify release access:

```bash
curl -I https://github.com/PSU3D0/agent-spreadsheet/releases/latest
```

### Unsupported platform

If your platform is not covered by prebuilt assets, build from source:

```bash
cargo install agent-spreadsheet --features recalc --bin asp --bin agent-spreadsheet
```

### Use a local binary in CI or development

```bash
AGENT_SPREADSHEET_LOCAL_BINARY=./target/release/agent-spreadsheet npm i -g agent-spreadsheet
```

### Corporate proxy / air-gapped environment

```bash
AGENT_SPREADSHEET_DOWNLOAD_BASE_URL=https://internal-mirror.example.com/releases npm i -g agent-spreadsheet
```

---

## How installation works

1. `npm install` runs `scripts/install.js`
2. the script detects your platform/architecture
3. it downloads the matching binary from GitHub Releases (or copies `AGENT_SPREADSHEET_LOCAL_BINARY`)
4. it places the binary in `vendor/`
5. `bin/agent-spreadsheet.js` launches it with your arguments

---

## See also

- Root project README: <https://github.com/PSU3D0/agent-spreadsheet#readme>
- MCP server crate: <https://github.com/PSU3D0/agent-spreadsheet/tree/main/crates/agent-spreadsheet-mcp>
- JS SDK: <https://github.com/PSU3D0/agent-spreadsheet/tree/main/npm/agent-spreadsheet-sdk>

---

## License

Apache-2.0
