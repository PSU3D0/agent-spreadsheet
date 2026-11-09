# Formualizer for Claude

Give Claude a real spreadsheet engine. Formualizer lets Claude recalculate, edit, inspect and trace Excel workbooks (`.xlsx`) directly, instead of guessing at formulas or asking you to open Excel.

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
codex plugin marketplace add https://github.com/psu3d0/formualizer-plugin.git
codex plugin add formualizer@formualizer-marketplace
```

Formualizer needs Node.js 20 or newer where Claude runs its code. Nothing else to install.

## Good to know

- Recalculating only updates stored results; the rest of the file stays byte-for-byte the same. Editing rewrites the workbook file.
- Formualizer writes files only where you ask it to. Your original workbook is never overwritten unless you name it as the output.

## What this plugin runs

The plugin includes the Formualizer spreadsheet engine as one compressed WebAssembly file (`skills/spreadsheet/engine/formualizer_engine.wasm.br`), together with readable JavaScript that loads it. When Claude uses the skill, Node.js checks the file against the SHA-256 recorded in `skills/spreadsheet/engine/PROVENANCE.json`, decompresses it with Node's built-in Brotli support, and runs it locally. `PROVENANCE.json` also records the source repository, commit and toolchain used to build the engine. The plugin installs no packages and downloads nothing.

## Privacy & Data Handling

Your workbooks never leave the environment where Claude runs the skill. The skill and its local server make no network requests, send no telemetry, and need no account or credentials. The platform you use Claude on (for example claude.ai or your own machine) handles your files under its own terms.

## Links

- Documentation: https://www.formualizer.dev/docs
- Engine source: https://github.com/PSU3D0/formualizer
- Issues and support: https://github.com/PSU3D0/formualizer/issues
- License: Apache-2.0
