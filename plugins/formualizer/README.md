# Formualizer Spreadsheet Plugin

Give your AI agent a real spreadsheet engine. Formualizer lets coding and desktop agents recalculate, edit, inspect and trace Excel workbooks (`.xlsx`) directly, instead of guessing at formulas or asking you to open Excel.

## What you can do

- **Fix stale numbers.** Workbooks produced by scripts and exports often carry outdated formula results. Formualizer recalculates them and writes a corrected copy, leaving everything else in the file untouched.
- **Edit workbooks safely.** Change inputs, values and formulas in one batch, recalculate, and get a receipt of exactly which cells changed.
- **Read a workbook accurately.** Look up cells, ranges, formulas and stored results on any sheet.
- **Find out where a number comes from.** Trace a cell back through every formula that feeds it, or forward to everything that depends on it.

## Try asking

- "Recalculate `forecast.xlsx` and tell me which totals changed."
- "Set the growth rate in `Inputs!B4` to 6%, recalculate, and show me the new year-5 revenue."
- "What feeds the NPV in `Summary!C12`? Walk me through it."
- "Which cells depend on the discount rate in `Inputs!B7`?"

## Install

**Claude Code**

```text
/plugin marketplace add PSU3D0/formualizer-plugin
/plugin install formualizer@formualizer-marketplace
```

**Codex CLI**

```sh
codex plugin marketplace add https://github.com/PSU3D0/formualizer-plugin.git
codex plugin add formualizer@formualizer-marketplace
```

**Gemini CLI**

```sh
gemini extensions install https://github.com/PSU3D0/formualizer-plugin
```

**Devin**

```sh
devin plugins install PSU3D0/formualizer-plugin
```

**Agent Skills (`skills.sh` — Claude Code, Cursor, Codex, Copilot, Windsurf, OpenClaw, Pi, and more)**

```sh
npx skills add PSU3D0/formualizer-plugin
```

**Local stdio MCP (any MCP client — zero cloud upload)**

```json
{
  "mcpServers": {
    "formualizer": {
      "command": "npx",
      "args": ["-y", "@formualizer/mcp"]
    }
  }
}
```

Formualizer needs Node.js 20 or newer where the agent runs its code. Nothing else to install.

## Good to know

- Recalculating only updates stored results; the rest of the file stays byte-for-byte the same. Editing rewrites the workbook file.
- Formualizer writes files only where you ask it to. Your original workbook is never overwritten unless you name it as the output.

## What this plugin runs

The plugin includes the Formualizer spreadsheet engine as one compressed WebAssembly file (`skills/spreadsheet/engine/formualizer_engine.wasm.br`), together with readable JavaScript that loads it. When invoked, Node.js checks the file against the SHA-256 recorded in `skills/spreadsheet/engine/PROVENANCE.json`, decompresses it with Node's built-in Brotli support, and runs it locally. `PROVENANCE.json` also records the source repository, commit and toolchain used to build the engine. The plugin installs no packages and downloads nothing.

## Privacy & Data Handling

Your workbooks never leave the environment where the agent runs the skill or local MCP server. The skill and its local server make no network requests, send no telemetry, and need no account or credentials. The host platform you run the agent on (for example claude.ai, Cursor, Codex, or your own machine) handles your files under its own terms.

## Links

- Documentation: https://www.formualizer.dev/docs
- Engine source: https://github.com/PSU3D0/formualizer
- Issues and support: https://github.com/PSU3D0/formualizer/issues
- License: Apache-2.0
