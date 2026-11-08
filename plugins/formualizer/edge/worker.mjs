// Stateless Cloudflare Edge Worker for Formualizer 4-verb XLSX Recalc, Edit, Inspect & Trace
// Exposes Streamable HTTP MCP (/mcp) + OpenAPI 3.1 (/openapi.json) + REST (/v1/*)
// Emits privacy-preserving anonymous structural telemetry (zero cell values, strings, or workbook bytes).

const PROTOCOL_VERSION = "2025-06-18";

export const EDGE_TOOLS = [
  {
    name: "source_recalculate",
    title: "Recalculate XLSX Formula Caches",
    description:
      "Recalculate cached formula values directly on .xlsx bytes using Formualizer. Returns calculation coverage, changed cache counts, error totals/samples, unknown functions, and optional recalculated workbook_base64.",
    annotations: {
      title: "Recalculate XLSX Formula Caches",
      readOnlyHint: true,
      destructiveHint: false,
      idempotentHint: true,
      openWorldHint: false
    },
    inputSchema: {
      type: "object",
      additionalProperties: false,
      required: ["workbook_base64"],
      properties: {
        workbook_base64: { type: "string", description: "Base64-encoded .xlsx bytes." },
        return_bytes: { type: "boolean", description: "Include recalculated .xlsx bytes as output_base64 (default: false)." },
        options: { type: "object", description: "Optional XlsxRecalculateOptions JSON." }
      }
    }
  },
  {
    name: "edit_xlsx",
    title: "Batch Edit & Recalculate XLSX",
    description:
      "Apply atomic cell/formula/name/structural edits to .xlsx bytes, recalculate with Formualizer, and return the edit + recalculation receipt (set details:true for exact diff) plus output_base64.",
    annotations: {
      title: "Batch Edit & Recalculate XLSX",
      readOnlyHint: false,
      destructiveHint: false,
      idempotentHint: true,
      openWorldHint: false
    },
    inputSchema: {
      type: "object",
      additionalProperties: false,
      required: ["workbook_base64", "request"],
      properties: {
        workbook_base64: { type: "string", description: "Base64-encoded source .xlsx bytes." },
        request: { type: "object", required: ["ops"], description: "StatelessEditRequest JSON ({ ops, recalculate?, details? })." },
        recalc_options: { type: "object", description: "Optional XlsxRecalculateOptions JSON." },
        return_bytes: { type: "boolean", description: "Include edited .xlsx bytes as output_base64 (default: true)." }
      }
    }
  },
  {
    name: "inspect_xlsx",
    title: "Inspect XLSX Cells & Cached Values",
    description:
      "Read-only inspection of source values, formulas, and unverified stored formula caches from .xlsx bytes.",
    annotations: {
      title: "Inspect XLSX Cells & Cached Values",
      readOnlyHint: true,
      destructiveHint: false,
      idempotentHint: true,
      openWorldHint: false
    },
    inputSchema: {
      type: "object",
      additionalProperties: false,
      required: ["workbook_base64", "request"],
      properties: {
        workbook_base64: { type: "string", description: "Base64-encoded .xlsx bytes." },
        request: { type: "object", required: ["selection"], description: "Inspect request ({ selection, page? })." }
      }
    }
  },
  {
    name: "trace_xlsx",
    title: "Trace Formula Precedents & Dependents",
    description:
      "Read-only dependency graph trace (precedents or dependents) rooted at one or more cells in .xlsx bytes.",
    annotations: {
      title: "Trace Formula Precedents & Dependents",
      readOnlyHint: true,
      destructiveHint: false,
      idempotentHint: true,
      openWorldHint: false
    },
    inputSchema: {
      type: "object",
      additionalProperties: false,
      required: ["workbook_base64", "request"],
      properties: {
        workbook_base64: { type: "string", description: "Base64-encoded .xlsx bytes." },
        request: { type: "object", required: ["roots"], description: "Trace request ({ roots, direction?, limits?, details? })." }
      }
    }
  }
];

function decodeBase64(b64) {
  if (typeof Buffer !== "undefined") {
    return new Uint8Array(Buffer.from(b64, "base64"));
  }
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

function encodeBase64(bytes) {
  if (typeof Buffer !== "undefined") {
    return Buffer.from(bytes).toString("base64");
  }
  let bin = "";
  for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
  return btoa(bin);
}

/**
 * Extracts strictly anonymous structural telemetry from the operation metadata.
 * Never includes cell values, string literals, sheet names, or workbook bytes.
 */
export function buildAnonymousTelemetry(operation, inputByteLength, metadata, elapsedMs, clientSurface = "unknown") {
  const recalc = metadata?.recalculation ?? metadata;
  const summary = recalc?.summary ?? null;
  const diag = recalc?.operationDiagnostics ?? null;
  const unknownFunctions = Array.isArray(diag?.unavailableFunctions)
    ? diag.unavailableFunctions.map(fn => ({ name: String(fn.name ?? ""), count: Number(fn.cells ?? fn.count ?? 0) }))
    : [];
  return {
    schema: "formualizer.edge.telemetry/1",
    operation,
    client_surface: clientSurface,
    input_bytes: inputByteLength,
    elapsed_ms: Math.round(elapsedMs * 100) / 100,
    coverage: recalc?.coverage ?? null,
    formula_cells: summary?.formulaCells ?? recalc?.formulaCells ?? null,
    evaluated_cells: summary?.evaluated ?? null,
    cache_cells_changed: recalc?.cacheCellsChanged ?? null,
    error_cells: recalc?.errorCells ?? summary?.errors ?? null,
    unknown_functions: unknownFunctions,
    workbook_features: Array.isArray(diag?.workbookFeatures) ? diag.workbookFeatures : []
  };
}

export function buildOpenApiSpec(serverUrl = "https://mcp.formualizer.com") {
  return {
    openapi: "3.1.0",
    info: {
      title: "Formualizer Spreadsheet Recalculation, Edit, Inspect & Trace API",
      version: "0.17.0",
      description: "Deterministic in-process Excel (.xlsx) recalculation, batch editing, source inspection, and formula dependency tracing powered by Formualizer."
    },
    servers: [{ url: serverUrl }],
    paths: {
      "/v1/recalc": {
        post: {
          operationId: "sourceRecalculate",
          summary: "Recalculate cached formula values in an .xlsx workbook",
          requestBody: {
            required: true,
            content: { "application/json": { schema: EDGE_TOOLS[0].inputSchema } }
          },
          responses: { "200": { description: "Recalculation receipt and optional output_base64" } }
        }
      },
      "/v1/edit": {
        post: {
          operationId: "editXlsx",
          summary: "Apply atomic edits and recalculate an .xlsx workbook",
          requestBody: {
            required: true,
            content: { "application/json": { schema: EDGE_TOOLS[1].inputSchema } }
          },
          responses: { "200": { description: "Edit and recalculation receipt plus output_base64" } }
        }
      },
      "/v1/inspect": {
        post: {
          operationId: "inspectXlsx",
          summary: "Inspect source values, formulas, and stored formula caches",
          requestBody: {
            required: true,
            content: { "application/json": { schema: EDGE_TOOLS[2].inputSchema } }
          },
          responses: { "200": { description: "Inspection report" } }
        }
      },
      "/v1/trace": {
        post: {
          operationId: "traceXlsx",
          summary: "Trace formula precedents or dependents",
          requestBody: {
            required: true,
            content: { "application/json": { schema: EDGE_TOOLS[3].inputSchema } }
          },
          responses: { "200": { description: "Trace report" } }
        }
      }
    }
  };
}

export function createEdgeRecalcWorker({ getRuntime, onTelemetry } = {}) {
  async function invokeTool(name, args = {}, clientSurface = "mcp") {
    const runtime = await getRuntime();
    const start = performance.now();
    const inputBytes = decodeBase64(args.workbook_base64 || "");
    if (name === "source_recalculate") {
      const optionsJson = args.options === undefined ? undefined : JSON.stringify(args.options);
      let res;
      try {
        res = runtime.recalculateSourceXlsx(inputBytes, optionsJson);
        const metadata = JSON.parse(res.metadataJson);
        const outBytes = res.bytes;
        const elapsedMs = performance.now() - start;
        const telemetry = buildAnonymousTelemetry("source_recalculate", inputBytes.byteLength, metadata, elapsedMs, clientSurface);
        if (onTelemetry) await onTelemetry(telemetry);
        return {
          mode: "recalculate",
          input_bytes: inputBytes.byteLength,
          result_bytes: outBytes.byteLength,
          metadata,
          output_base64: args.return_bytes ? encodeBase64(outBytes) : undefined
        };
      } finally {
        res?.free?.();
      }
    }
    if (name === "edit_xlsx") {
      const requestJson = typeof args.request === "string" ? args.request : JSON.stringify(args.request);
      const recalcOptionsJson = args.recalc_options === undefined
        ? undefined
        : typeof args.recalc_options === "string"
          ? args.recalc_options
          : JSON.stringify(args.recalc_options);
      let res;
      try {
        res = runtime.editXlsx(inputBytes, requestJson, recalcOptionsJson);
        const metadata = JSON.parse(res.metadataJson);
        const outBytes = res.bytes;
        const elapsedMs = performance.now() - start;
        const telemetry = buildAnonymousTelemetry("edit_xlsx", inputBytes.byteLength, metadata, elapsedMs, clientSurface);
        if (onTelemetry) await onTelemetry(telemetry);
        return {
          mode: "edit",
          input_bytes: inputBytes.byteLength,
          result_bytes: outBytes.byteLength,
          metadata,
          output_base64: args.return_bytes !== false ? encodeBase64(outBytes) : undefined
        };
      } finally {
        res?.free?.();
      }
    }
    if (name === "inspect_xlsx" || name === "trace_xlsx") {
      const requestJson = typeof args.request === "string" ? args.request : JSON.stringify(args.request);
      const method = name === "trace_xlsx" ? "traceXlsx" : "inspectXlsx";
      let res;
      try {
        res = runtime[method](inputBytes, requestJson);
        const report = JSON.parse(res.metadataJson);
        const elapsedMs = performance.now() - start;
        const telemetry = buildAnonymousTelemetry(name, inputBytes.byteLength, report, elapsedMs, clientSurface);
        if (onTelemetry) await onTelemetry(telemetry);
        return {
          mode: name === "trace_xlsx" ? "trace" : "inspect",
          report
        };
      } finally {
        res?.free?.();
      }
    }
    throw new Error(`Unknown tool: ${name}`);
  }

  return {
    invokeTool,
    async fetch(request) {
      const url = new URL(request.url);
      const clientSurface = request.headers.get("x-formualizer-client") || request.headers.get("user-agent") || "http";
      if (request.method === "GET" && url.pathname === "/health") {
        return Response.json({ ok: true, service: "formualizer-edge-recalc", version: "0.17.0" });
      }
      if (request.method === "GET" && url.pathname === "/openapi.json") {
        return Response.json(buildOpenApiSpec(url.origin));
      }
      const restMap = {
        "/v1/recalc": "source_recalculate",
        "/v1/edit": "edit_xlsx",
        "/v1/inspect": "inspect_xlsx",
        "/v1/trace": "trace_xlsx"
      };
      if (request.method === "POST" && restMap[url.pathname]) {
        try {
          const body = await request.json();
          const result = await invokeTool(restMap[url.pathname], body, clientSurface);
          return Response.json(result);
        } catch (err) {
          return Response.json(
            { error: typeof err === "object" && err !== null ? err : { message: String(err) } },
            { status: 400 }
          );
        }
      }
      if (request.method === "POST" && url.pathname === "/mcp") {
        const msg = await request.json();
        if (msg.method === "initialize") {
          return Response.json({
            jsonrpc: "2.0",
            id: msg.id,
            result: {
              protocolVersion: msg.params?.protocolVersion || PROTOCOL_VERSION,
              capabilities: { tools: { listChanged: false } },
              serverInfo: { name: "formualizer-edge-recalc", version: "0.17.0" }
            }
          });
        }
        if (msg.method === "tools/list") {
          return Response.json({
            jsonrpc: "2.0",
            id: msg.id,
            result: { tools: EDGE_TOOLS }
          });
        }
        if (msg.method === "tools/call") {
          try {
            const payload = await invokeTool(msg.params?.name, msg.params?.arguments ?? {}, clientSurface);
            return Response.json({
              jsonrpc: "2.0",
              id: msg.id,
              result: {
                content: [{ type: "text", text: JSON.stringify(payload) }],
                structuredContent: payload,
                isError: false
              }
            });
          } catch (err) {
            return Response.json({
              jsonrpc: "2.0",
              id: msg.id,
              result: {
                content: [{ type: "text", text: typeof err === "object" ? JSON.stringify(err) : String(err) }],
                isError: true
              }
            });
          }
        }
        return Response.json({
          jsonrpc: "2.0",
          id: msg.id ?? null,
          error: { code: -32601, message: `Method not found: ${msg.method}` }
        });
      }
      return new Response("Not Found", { status: 404 });
    }
  };
}
