import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, writeFile, rm, access, readdir, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { handleJsonRpcMessage } from "../../../server/stdio-mcp.mjs";
import { createEdgeRecalcWorker, runtimeFromModule } from "../../../edge/worker.mjs";
import { loadRuntime } from "../scripts/xlsx.mjs";
import { brotliDecompressSync } from "node:zlib";
import { syntheticWorkbook } from "../scripts/synthetic.mjs";

const skillRoot = fileURLToPath(new URL("../", import.meta.url));
const pluginRoot = fileURLToPath(new URL("../../../", import.meta.url));
const loader = process.env.FORMUALIZER_TEST_RUNTIME;
const real = (name, fn) => test(name, fn);

async function temporary(t) {
  const dir = await mkdtemp(join(tmpdir(), "formualizer-mcp-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  return dir;
}

test("all multi-platform manifests exist, parse cleanly, and have no top-level bin/ or oversized files", async () => {
  await assert.rejects(access(join(pluginRoot, "bin")));
  const manifests = [
    "plugin.json",
    "mcp.json",
    ".cursor-plugin/plugin.json",
    "gemini-extension.json",
    ".devin-plugin/plugin.json",
    "server.json",
    "glama.json"
  ];
  for (const rel of manifests) {
    const raw = await readFile(join(pluginRoot, rel), "utf8");
    const parsed = JSON.parse(raw);
    assert.ok(parsed && typeof parsed === "object", rel);
  }

  async function walkSizes(dir) {
    for (const entry of await readdir(dir, { withFileTypes: true })) {
      if (entry.name === "node_modules" || entry.name === ".git") continue;
      const full = join(dir, entry.name);
      if (entry.isDirectory()) await walkSizes(full);
      else {
        const info = await stat(full);
        assert.ok(info.size < 5 * 1024 * 1024, `File reaches 5 MiB limit: ${full}`);
        if (info.size > 256 * 1024) assert.equal(entry.name, "formualizer_engine.wasm.br");
      }
    }
  }
  await walkSizes(pluginRoot);
});

test("stdio-mcp initializes and lists exactly the 4 narrow tools with readOnlyHint/destructiveHint annotations", async () => {
  const init = await handleJsonRpcMessage({
    jsonrpc: "2.0",
    id: 1,
    method: "initialize",
    params: { protocolVersion: "2025-06-18" }
  });
  assert.equal(init.result.serverInfo.name, "formualizer-spreadsheet");

  const list = await handleJsonRpcMessage({
    jsonrpc: "2.0",
    id: 2,
    method: "tools/list"
  });
  const names = list.result.tools.map(t => t.name);
  assert.deepEqual(names, ["source_recalculate", "edit_xlsx", "inspect_xlsx", "trace_xlsx"]);
  for (const tool of list.result.tools) {
    assert.equal(typeof tool.annotations?.title, "string");
    assert.equal(typeof tool.annotations?.readOnlyHint, "boolean");
    assert.equal(typeof tool.annotations?.destructiveHint, "boolean");
  }
  assert.equal(list.result.tools[2].annotations.readOnlyHint, true);
  assert.equal(list.result.tools[3].annotations.readOnlyHint, true);
});

real("stdio-mcp and edge worker execute all 4 narrow verbs and emit zero-PII structural telemetry", async t => {
  const dir = await temporary(t);
  const input = join(dir, "input.xlsx");
  const edited = join(dir, "edited.xlsx");
  const sheetXml = '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><v>21</v></c><c r="B1"><f>A1*2</f><v>0</v></c></row></sheetData></worksheet>';
  const wbBytes = syntheticWorkbook({ "xl/worksheets/sheet1.xml": sheetXml });
  await writeFile(input, wbBytes);

  // 1. stdio-mcp source_recalculate check
  const recalcResp = await handleJsonRpcMessage({
    jsonrpc: "2.0",
    id: 10,
    method: "tools/call",
    params: {
      name: "source_recalculate",
      arguments: { input_path: input }
    }
  }, loader);
  assert.equal(recalcResp.result.isError, false);
  assert.equal(recalcResp.result.structuredContent.metadata.cacheCellsChanged, 1);

  // 2. stdio-mcp edit_xlsx with details: true
  const editResp = await handleJsonRpcMessage({
    jsonrpc: "2.0",
    id: 11,
    method: "tools/call",
    params: {
      name: "edit_xlsx",
      arguments: {
        input_path: input,
        output_path: edited,
        request: {
          ops: [{ kind: "set_cells", sheet_name: "Sheet1", cells: { A1: { kind: "value", value: 30 } } }],
          recalculate: "always",
          details: true
        }
      }
    }
  }, loader);
  assert.equal(editResp.result.isError, false);
  assert.equal(editResp.result.structuredContent.metadata.edit.ops_applied, 1);
  assert.equal(editResp.result.structuredContent.metadata.edit.diff_exact, true);
  assert.equal(editResp.result.structuredContent.metadata.recalculation.summary.evaluated, 1);

  // 3. stdio-mcp inspect_xlsx & trace_xlsx
  const inspectResp = await handleJsonRpcMessage({
    jsonrpc: "2.0",
    id: 12,
    method: "tools/call",
    params: {
      name: "inspect_xlsx",
      arguments: {
        input_path: edited,
        request: { selection: { kind: "cells", addresses: ["Sheet1!B1"] } }
      }
    }
  }, loader);
  assert.equal(inspectResp.result.isError, false);
  assert.equal(inspectResp.result.structuredContent.report.items[0].stored_formula_cache.value.value, 60);

  const traceResp = await handleJsonRpcMessage({
    jsonrpc: "2.0",
    id: 13,
    method: "tools/call",
    params: {
      name: "trace_xlsx",
      arguments: {
        input_path: edited,
        request: { roots: ["Sheet1!B1"] }
      }
    }
  }, loader);
  assert.equal(traceResp.result.isError, false);
  assert.equal(traceResp.result.structuredContent.report.items[0].cell.address, "Sheet1!B1");

  // 4. Cloudflare Edge Worker REST + MCP + Anonymous Telemetry
  const telemetryEvents = [];
  const worker = createEdgeRecalcWorker({
    getRuntime: async () => (await loadRuntime(loader)).runtime,
    onTelemetry: async ev => telemetryEvents.push(ev)
  });

  const openApiRes = await worker.fetch(new Request("https://mcp.formualizer.com/openapi.json"));
  assert.equal(openApiRes.status, 200);
  const openApi = await openApiRes.json();
  assert.equal(openApi.openapi, "3.1.0");
  assert.ok(openApi.paths["/v1/recalc"]);

  const mcpCallRes = await worker.fetch(
    new Request("https://mcp.formualizer.com/mcp", {
      method: "POST",
      headers: { "content-type": "application/json", "x-formualizer-client": "chatgpt-mcp" },
      body: JSON.stringify({
        jsonrpc: "2.0",
        id: 99,
        method: "tools/call",
        params: {
          name: "source_recalculate",
          arguments: {
            workbook_base64: Buffer.from(wbBytes).toString("base64"),
            return_bytes: true
          }
        }
      })
    })
  );
  assert.equal(mcpCallRes.status, 200);
  const mcpCall = await mcpCallRes.json();
  assert.equal(mcpCall.result.isError, false);
  assert.equal(mcpCall.result.structuredContent.metadata.cacheCellsChanged, 1);
  assert.ok(mcpCall.result.structuredContent.output_base64.length > 50);

  assert.equal(telemetryEvents.length, 1);
  assert.equal(telemetryEvents[0].schema, "formualizer.edge.telemetry/1");
  assert.equal(telemetryEvents[0].operation, "source_recalculate");
  assert.equal(telemetryEvents[0].client_surface, "chatgpt-mcp");
  assert.equal(telemetryEvents[0].cache_cells_changed, 1);
  const serializedTelemetry = JSON.stringify(telemetryEvents[0]);
  assert.ok(!serializedTelemetry.includes("Sheet1"), "Telemetry must not leak sheet names");
  assert.ok(!serializedTelemetry.includes("21"), "Telemetry must not leak cell values");
});

test("edge accepts a build-time compiled module and defaults to the bundled Node engine", async () => {
  const raw = brotliDecompressSync(await readFile(join(skillRoot, "engine/formualizer_engine.wasm.br")));
  const runtime = runtimeFromModule(new WebAssembly.Module(raw));
  assert.equal(JSON.parse(runtime.capabilities()).sessions, false);
  const worker = createEdgeRecalcWorker();
  const response = await worker.invokeTool("inspect_xlsx", {
    workbook_base64: Buffer.from(syntheticWorkbook()).toString("base64"),
    request: { selection: { kind: "cells", addresses: ["Sheet1!A1"] } }
  });
  assert.equal(response.report.items[0].source_value.value, 21);
});
