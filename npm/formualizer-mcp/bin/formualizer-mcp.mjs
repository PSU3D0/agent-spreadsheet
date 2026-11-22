#!/usr/bin/env node
import { resolve } from "node:path";
import { handleJsonRpcMessage } from "../dist/server/stdio-mcp.mjs";

const args = process.argv.slice(2);
if (args.includes("--help") || args.includes("-h")) {
  process.stderr.write(
    [
      "Usage: formualizer-mcp [--dev-runtime <path>]",
      "",
      "Local-first stdio MCP server for Excel (.xlsx) recalculation, batch editing,",
      "inspection, and formula dependency tracing powered by bundled Formualizer WASM.",
      "Zero network requests; workbooks never leave your machine."
    ].join("\n") + "\n"
  );
  process.exit(0);
}

let devRuntime;
const devIdx = args.indexOf("--dev-runtime");
if (devIdx !== -1 && args[devIdx + 1]) {
  devRuntime = resolve(args[devIdx + 1]);
}

let buffer = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", async chunk => {
  buffer += chunk;
  const lines = buffer.split("\n");
  buffer = lines.pop() ?? "";
  for (const line of lines) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    try {
      const parsed = JSON.parse(trimmed);
      const response = await handleJsonRpcMessage(parsed, devRuntime);
      if (response !== null) {
        process.stdout.write(JSON.stringify(response) + "\n");
      }
    } catch (err) {
      process.stdout.write(
        JSON.stringify({
          jsonrpc: "2.0",
          id: null,
          error: { code: -32700, message: `Parse error: ${err.message}` }
        }) + "\n"
      );
    }
  }
});
