import { readFile, realpath } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { loadRuntime, recalculateFile, editFile, inspectFile } from "../skills/spreadsheet/scripts/xlsx.mjs";

const PROTOCOL_VERSION = "2025-06-18";

const TOOLS = [
  {
    name: "source_recalculate",
    title: "Recalculate XLSX Formula Caches",
    description:
      "Recalculate cached formula values directly on original .xlsx bytes using Formualizer without resident sessions. Returns calculation coverage, changed cache counts, error totals/samples, and unknown functions.",
    annotations: {
      title: "Recalculate XLSX Formula Caches",
      readOnlyHint: false,
      destructiveHint: false,
      idempotentHint: true,
      openWorldHint: false
    },
    inputSchema: {
      type: "object",
      additionalProperties: false,
      required: ["input_path"],
      properties: {
        input_path: {
          type: "string",
          description: "Path to the input .xlsx file."
        },
        output_path: {
          type: "string",
          description: "Optional destination .xlsx path. Omit for read-only check mode."
        },
        options: {
          type: "object",
          description: "Optional calculation options (profile: 'workbook' | 'strict', requireComplete, calculation, errorLocationLimit, externalLinks, limits)."
        }
      }
    }
  },
  {
    name: "edit_xlsx",
    title: "Batch Edit & Recalculate XLSX",
    description:
      "Apply atomic cell/formula/name/structural edits to an .xlsx file, optionally recalculate formula caches with Formualizer, write output_path, and return the canonical edit + recalculation receipt (set details:true in request for exact cell diff).",
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
      required: ["input_path", "output_path", "request"],
      properties: {
        input_path: {
          type: "string",
          description: "Path to the source .xlsx file."
        },
        output_path: {
          type: "string",
          description: "Path to write the edited and recalculated .xlsx file."
        },
        request: {
          type: "object",
          required: ["ops"],
          description: "StatelessEditRequest JSON object: { ops: [...], recalculate?: 'auto' | 'always' | 'never', details?: boolean, formula_parse_policy?: 'warn' | 'fail' | 'off' }."
        },
        recalc_options: {
          type: "object",
          description: "Optional XlsxRecalculateOptions JSON object."
        }
      }
    }
  },
  {
    name: "inspect_xlsx",
    title: "Inspect XLSX Cells & Cached Values",
    description:
      "Read-only inspection of source values, formulas, and unverified stored formula caches from an .xlsx file without loading a resident session.",
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
      required: ["input_path", "request"],
      properties: {
        input_path: {
          type: "string",
          description: "Path to the .xlsx file to inspect."
        },
        request: {
          type: "object",
          required: ["selection"],
          description: "Inspect request object: { selection: { kind: 'cells', addresses: ['Sheet1!A1'] } | { kind: 'range', address: 'Sheet1!A1:D20' }, page?: { limit?: number, offset?: number, expected_fingerprint?: string } }."
        }
      }
    }
  },
  {
    name: "trace_xlsx",
    title: "Trace Formula Precedents & Dependents",
    description:
      "Read-only dependency graph trace (precedents or dependents) rooted at one or more cells in an .xlsx file.",
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
      required: ["input_path", "request"],
      properties: {
        input_path: {
          type: "string",
          description: "Path to the .xlsx file to trace."
        },
        request: {
          type: "object",
          required: ["roots"],
          description: "Trace request object: { roots: ['Sheet1!B1'], direction?: 'precedents' | 'dependents', limits?: { max_depth?: number, max_nodes?: number, max_links?: number }, details?: boolean }."
        }
      }
    }
  }
];

export async function dispatchToolCall(name, args = {}, devRuntime) {
  const { runtime } = await loadRuntime(devRuntime);
  if (name === "source_recalculate") {
    const optionsJson = args.options === undefined ? undefined : JSON.stringify(args.options);
    const receipt = await recalculateFile(runtime, args.input_path, optionsJson, args.output_path);
    return {
      mode: args.output_path === undefined ? "check" : "recalculate",
      ...receipt
    };
  }
  if (name === "edit_xlsx") {
    const requestJson = typeof args.request === "string" ? args.request : JSON.stringify(args.request);
    const recalcOptionsJson = args.recalc_options === undefined
      ? undefined
      : typeof args.recalc_options === "string"
        ? args.recalc_options
        : JSON.stringify(args.recalc_options);
    return {
      mode: "edit",
      ...(await editFile(runtime, args.input_path, requestJson, args.output_path, recalcOptionsJson))
    };
  }
  if (name === "inspect_xlsx" || name === "trace_xlsx") {
    const requestJson = typeof args.request === "string" ? args.request : JSON.stringify(args.request);
    const report = await inspectFile(runtime, args.input_path, requestJson, name === "trace_xlsx");
    return {
      mode: name === "trace_xlsx" ? "trace" : "inspect",
      report
    };
  }
  throw new Error(`Unknown tool: ${name}`);
}

export async function handleJsonRpcMessage(message, devRuntime) {
  if (!message || typeof message !== "object" || message.jsonrpc !== "2.0") {
    return {
      jsonrpc: "2.0",
      id: message?.id ?? null,
      error: { code: -32600, message: "Invalid JSON-RPC 2.0 request" }
    };
  }
  const { id, method, params } = message;
  if (method === "notifications/initialized" || method === "notifications/cancelled") {
    return null;
  }
  if (method === "ping") {
    return { jsonrpc: "2.0", id, result: {} };
  }
  if (method === "initialize") {
    return {
      jsonrpc: "2.0",
      id,
      result: {
        protocolVersion: params?.protocolVersion || PROTOCOL_VERSION,
        capabilities: { tools: { listChanged: false } },
        serverInfo: {
          name: "formualizer-spreadsheet",
          version: "0.18.0"
        }
      }
    };
  }
  if (method === "tools/list") {
    return {
      jsonrpc: "2.0",
      id,
      result: { tools: TOOLS }
    };
  }
  if (method === "tools/call") {
    try {
      const toolName = params?.name;
      const toolArgs = params?.arguments ?? {};
      const payload = await dispatchToolCall(toolName, toolArgs, devRuntime);
      return {
        jsonrpc: "2.0",
        id,
        result: {
          content: [{ type: "text", text: JSON.stringify(payload, null, 2) }],
          structuredContent: payload,
          isError: false
        }
      };
    } catch (error) {
      const text = typeof error === "string"
        ? error
        : error instanceof Error
          ? error.message
          : JSON.stringify(error);
      return {
        jsonrpc: "2.0",
        id,
        result: {
          content: [{ type: "text", text }],
          isError: true
        }
      };
    }
  }
  return {
    jsonrpc: "2.0",
    id: id ?? null,
    error: { code: -32601, message: `Method not found: ${method}` }
  };
}

// Compare full resolved paths so symlinked launches work and imports stay inert.
const invocationPath = process.argv[1] ? await realpath(process.argv[1]).catch(() => null) : null;
if (invocationPath && import.meta.url === pathToFileURL(invocationPath).href) {
  const args = process.argv.slice(2);
  let devRuntime;
  if (args[0] === "--dev-runtime" && args[1]) {
    devRuntime = resolve(args[1]);
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
}
