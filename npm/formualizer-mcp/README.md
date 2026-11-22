# `@formualizer/mcp`

Local-first stdio MCP server for Excel (`.xlsx`) formula recalculation, batch editing, source inspection, and precedent/dependent tracing — powered by a bundled, SHA-256 verified Formualizer WebAssembly engine.

- **Zero cloud upload:** Workbooks stay on your machine. No network calls, no telemetry, no account, no Docker, no LibreOffice, no Python virtualenv.
- **4 focused tools:** `source_recalculate`, `edit_xlsx`, `inspect_xlsx`, `trace_xlsx`.
- **Requirements:** Node.js `>= 20`.

## Quick Start

Add to Claude Desktop, Cursor, VS Code Copilot, Windsurf, Codex, or any MCP client:

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

## Tools

| Tool | Read-only | Purpose |
|---|---:|---|
| `source_recalculate` | check mode / write copy | Recalculate cached formula values directly on original `.xlsx` bytes while preserving non-cache parts. |
| `edit_xlsx` | no | Apply atomic cell/formula/name/structural edits, recalculate, and return a change receipt. |
| `inspect_xlsx` | yes | Inspect source values, formulas, and stored formula caches for targeted cells or finite ranges. |
| `trace_xlsx` | yes | Trace formula precedents or dependents rooted at one or more cells. |

## Links

- Plugin repository: https://github.com/PSU3D0/formualizer-plugin
- Core engine: https://github.com/PSU3D0/formualizer
- Documentation: https://www.formualizer.dev/docs
- License: Apache-2.0
