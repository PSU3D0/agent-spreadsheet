import { readFile, writeFile, realpath } from "node:fs/promises";
import { loadEngine } from "./engine.mjs";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { createHash } from "node:crypto";

export async function loadRuntime(developmentModule) {
  if (Number(process.versions.node.split(".")[0]) < 20) throw new Error("Node >=20 required.");
  if (developmentModule !== undefined) {
    if (typeof developmentModule !== "string" || !developmentModule || /^[a-z][a-z0-9+.-]*:/i.test(developmentModule))
      throw new Error("--dev-runtime requires an explicit local loader path, not a URL.");
    const local = resolve(developmentModule);
    const loader = await import(pathToFileURL(local).href);
    return { runtime: await loader.createWasmRuntime(), version: null, development_module: local };
  }
  return loadEngine();
}

const digest = bytes => createHash("sha256").update(bytes).digest("hex");

// Uses original bytes directly. Workbook semantics and options belong entirely
// to the binding; this transport only owns file I/O and result lifetime.
export async function recalculateFile(runtime, input, optionsJson, output) {
  if (typeof runtime.recalculateSourceXlsx !== "function") {
    throw new Error("Runtime does not export recalculateSourceXlsx. Use the bundled matching engine.");
  }
  const receipt = await byteFile(input, output, bytes => runtime.recalculateSourceXlsx(bytes, optionsJson));
  // Legacy current means byte identity only; it makes no freshness claim.
  return { ...receipt, bytesUnchanged: receipt.bytes_unchanged, current: receipt.bytes_unchanged };
}

export async function editFile(runtime, input, requestJson, output, recalcOptionsJson) {
  if (typeof runtime.editXlsx !== "function") {
    throw new Error("Runtime does not export editXlsx. Use the bundled matching engine.");
  }
  return byteFile(input, output, bytes => runtime.editXlsx(bytes, requestJson, recalcOptionsJson));
}

async function byteFile(input, output, invoke) {
  const original = await readFile(input);
  let result;
  try {
    result = await invoke(new Uint8Array(original));
    if (typeof result?.free !== "function") throw new Error("Byte binding result must expose free().");
    const typedBytes = result.bytes;
    const rawMetadata = result.metadataJson;
    if (!(typedBytes instanceof Uint8Array) || typeof rawMetadata !== "string") {
      throw new Error("Byte binding must return typed bytes separately from metadataJson.");
    }
    const bytes = Buffer.from(typedBytes);
    const metadata = JSON.parse(rawMetadata);
    const receipt = {
      input: resolve(input), output: output === undefined ? null : resolve(output),
      bytes_unchanged: bytes.equals(original), input_sha256: digest(original),
      result_sha256: digest(bytes), result_bytes: bytes.length, metadata
    };
    if (output !== undefined) await writeFile(output, bytes);
    return receipt;
  } finally {
    if (typeof result?.free === "function") result.free();
  }
}

function parsePositiveInt(flag, raw) {
  if (typeof raw !== "string" || !/^[1-9]\d*$/.test(raw)) {
    throw new Error(`${flag} requires a positive integer.`);
  }
  const value = Number(raw);
  if (!Number.isSafeInteger(value)) throw new Error(`${flag} exceeds safe integer range.`);
  return value;
}

function parseNonNegativeInt(flag, raw) {
  if (typeof raw !== "string" || !/^\d+$/.test(raw)) {
    throw new Error(`${flag} requires a non-negative integer.`);
  }
  const value = Number(raw);
  if (!Number.isSafeInteger(value)) throw new Error(`${flag} exceeds safe integer range.`);
  return value;
}

function sourceArguments(args) {
  const [input, ...flags] = args;
  if (!input || input.startsWith("--")) throw new Error("source-recalculate requires INPUT.xlsx.");
  const options = { input, check: false };
  for (let index = 0; index < flags.length; index++) {
    const flag = flags[index];
    if (flag === "--check" && !options.check) options.check = true;
    else if ((flag === "--options" || flag === "--output") && flags[index + 1] &&
             !flags[index + 1].startsWith("--")) {
      const key = flag === "--options" ? "options" : "output";
      if (options[key] !== undefined || (key === "options" && options.optionsJson !== undefined)) {
        throw new Error("Duplicate source argument: " + flag);
      }
      options[key] = flags[++index];
    } else if (flag === "--options-json" && index + 1 < flags.length) {
      if (options.options !== undefined || options.optionsJson !== undefined) {
        throw new Error("Duplicate source argument: " + flag);
      }
      options.optionsJson = flags[++index];
    } else throw new Error("Invalid source argument: " + flag);
  }
  if (options.check && options.output !== undefined) throw new Error("--check cannot be combined with --output.");
  return options;
}

function editArguments(args) {
  const [input, ...flags] = args;
  if (!input || input.startsWith("--")) throw new Error("edit requires INPUT.xlsx.");
  const parsed = { input };
  for (let index = 0; index < flags.length; index++) {
    const flag = flags[index];
    if (flag === "--operations-json" || flag === "--recalc-options-json") {
      const key = flag === "--operations-json" ? "operationsJson" : "recalcOptionsJson";
      const fileKey = flag === "--operations-json" ? "operations" : "recalcOptions";
      if (parsed[key] !== undefined || parsed[fileKey] !== undefined || index + 1 >= flags.length) {
        throw new Error("Invalid edit argument: " + flag);
      }
      parsed[key] = flags[++index];
      continue;
    }
    const key = { "--operations": "operations", "--output": "output", "--recalc-options": "recalcOptions" }[flag];
    const jsonKey = flag === "--operations" ? "operationsJson" : flag === "--recalc-options" ? "recalcOptionsJson" : null;
    if (!key || parsed[key] !== undefined || (jsonKey && parsed[jsonKey] !== undefined) ||
        !flags[index + 1] || flags[index + 1].startsWith("--")) {
      throw new Error("Invalid edit argument: " + flag);
    }
    parsed[key] = flags[++index];
  }
  if ((!parsed.operations && parsed.operationsJson === undefined) || !parsed.output) {
    throw new Error("edit requires --operations REQUEST.json (or --operations-json JSON) and --output OUTPUT.xlsx.");
  }
  return parsed;
}

function inspectArguments(args) {
  const [input, ...rest] = args;
  if (!input || input.startsWith("--") || rest.length === 0) {
    throw new Error("inspect requires INPUT.xlsx followed by REQUEST.json, --json JSON, --range Sheet!A1:D20, or --cells Sheet!A1 ...");
  }
  if (!rest[0].startsWith("--")) {
    if (rest.length !== 1) {
      throw new Error("inspect positional REQUEST.json cannot be combined with extra arguments.");
    }
    return { input, requestPath: rest[0] };
  }
  let requestPath, inlineJson, range, limit;
  const cells = [];
  for (let index = 0; index < rest.length; index++) {
    const flag = rest[index];
    if (flag === "--request" && rest[index + 1] && !rest[index + 1].startsWith("--") && requestPath === undefined) {
      requestPath = rest[++index];
    } else if (flag === "--json" && index + 1 < rest.length && inlineJson === undefined) {
      inlineJson = rest[++index];
    } else if (flag === "--range" && rest[index + 1] && !rest[index + 1].startsWith("--") && range === undefined) {
      range = rest[++index];
    } else if (flag === "--cell" && rest[index + 1] && !rest[index + 1].startsWith("--")) {
      cells.push(rest[++index]);
    } else if (flag === "--cells") {
      let count = 0;
      while (index + 1 < rest.length && !rest[index + 1].startsWith("--")) {
        cells.push(rest[++index]);
        count++;
      }
      if (count === 0) throw new Error("--cells requires at least one sheet-qualified cell address (e.g. Sheet1!A1).");
    } else if (flag === "--limit" && index + 1 < rest.length && limit === undefined) {
      limit = parsePositiveInt("--limit", rest[++index]);
    } else {
      throw new Error("Invalid inspect argument: " + flag + ". Use REQUEST.json, --json JSON, --range Sheet!A1:D20, --cells Sheet!A1 ..., or --limit N.");
    }
  }
  const selectors = Number(requestPath !== undefined) + Number(inlineJson !== undefined) +
    Number(range !== undefined) + Number(cells.length > 0);
  if (selectors !== 1) {
    throw new Error("inspect requires exactly one selector: REQUEST.json, --request FILE.json, --json JSON, --range Sheet!A1:D20, or --cells Sheet!A1 ...");
  }
  if (limit !== undefined && (requestPath !== undefined || inlineJson !== undefined)) {
    throw new Error("--limit only combines with --range or --cells; set page.limit inside JSON requests.");
  }
  if (requestPath !== undefined) return { input, requestPath };
  if (inlineJson !== undefined) return { input, inlineJson };
  const body = range !== undefined
    ? { selection: { kind: "range", address: range } }
    : { selection: { kind: "cells", addresses: cells } };
  if (limit !== undefined) body.page = { limit };
  return { input, inlineJson: JSON.stringify(body) };
}

function traceArguments(args) {
  const [input, ...rest] = args;
  if (!input || input.startsWith("--") || rest.length === 0) {
    throw new Error("trace requires INPUT.xlsx followed by REQUEST.json, --json JSON, or --roots Sheet!B1 ...");
  }
  if (!rest[0].startsWith("--")) {
    if (rest.length !== 1) {
      throw new Error("trace positional REQUEST.json cannot be combined with extra arguments.");
    }
    return { input, requestPath: rest[0] };
  }
  let requestPath, inlineJson, direction, maxDepth, limit, details = false;
  const roots = [];
  for (let index = 0; index < rest.length; index++) {
    const flag = rest[index];
    if (flag === "--request" && rest[index + 1] && !rest[index + 1].startsWith("--") && requestPath === undefined) {
      requestPath = rest[++index];
    } else if (flag === "--json" && index + 1 < rest.length && inlineJson === undefined) {
      inlineJson = rest[++index];
    } else if (flag === "--root" && rest[index + 1] && !rest[index + 1].startsWith("--")) {
      roots.push(rest[++index]);
    } else if (flag === "--roots") {
      let count = 0;
      while (index + 1 < rest.length && !rest[index + 1].startsWith("--")) {
        roots.push(rest[++index]);
        count++;
      }
      if (count === 0) throw new Error("--roots requires at least one sheet-qualified cell address (e.g. Sheet1!B1).");
    } else if (flag === "--direction" && rest[index + 1] && direction === undefined) {
      const value = rest[++index];
      if (value !== "precedents" && value !== "dependents") {
        throw new Error("--direction must be precedents or dependents.");
      }
      direction = value;
    } else if (flag === "--max-depth" && index + 1 < rest.length && maxDepth === undefined) {
      maxDepth = parseNonNegativeInt("--max-depth", rest[++index]);
    } else if (flag === "--limit" && index + 1 < rest.length && limit === undefined) {
      limit = parsePositiveInt("--limit", rest[++index]);
    } else if (flag === "--details" && !details) {
      details = true;
    } else {
      throw new Error("Invalid trace argument: " + flag + ". Use REQUEST.json, --json JSON, --roots Sheet!B1 ..., --direction precedents|dependents, --max-depth N, --limit N, or --details.");
    }
  }
  const selectors = Number(requestPath !== undefined) + Number(inlineJson !== undefined) + Number(roots.length > 0);
  if (selectors !== 1) {
    throw new Error("trace requires exactly one selector: REQUEST.json, --request FILE.json, --json JSON, or --roots Sheet!B1 ...");
  }
  if ((requestPath !== undefined || inlineJson !== undefined) &&
      (direction !== undefined || maxDepth !== undefined || limit !== undefined || details)) {
    throw new Error("Trace flags (--direction, --max-depth, --limit, --details) only combine with --roots; set those fields inside JSON requests.");
  }
  if (requestPath !== undefined) return { input, requestPath };
  if (inlineJson !== undefined) return { input, inlineJson };
  const body = { roots };
  if (direction !== undefined) body.direction = direction;
  if (maxDepth !== undefined) body.limits = { max_depth: maxDepth };
  if (limit !== undefined) body.page = { limit };
  if (details) body.details = true;
  return { input, inlineJson: JSON.stringify(body) };
}

export async function inspectFile(runtime, input, requestJson, trace = false) {
  const method = trace ? "traceXlsx" : "inspectXlsx";
  if (typeof runtime[method] !== "function") throw new Error(method + " requires an explicitly selected matching development runtime.");
  const bytes = await readFile(input);
  let result;
  try {
    result = runtime[method](new Uint8Array(bytes), requestJson);
    return JSON.parse(result.metadataJson);
  } finally { result?.free(); }
}

export async function main(args) {
  let developmentModule;
  if (args[0] === "--dev-runtime") {
    if (!args[1]) throw new Error("--dev-runtime requires a local loader path.");
    developmentModule = args[1];
    args = args.slice(2);
  }
  const [command, ...rest] = args;
  if (command === "inspect" || command === "trace") {
    const parsed = command === "trace" ? traceArguments(rest) : inspectArguments(rest);
    const loaded = await loadRuntime(developmentModule);
    const requestJson = parsed.inlineJson ?? await readFile(parsed.requestPath, "utf8");
    return { mode: command,
      runtime: loaded.development_module
        ? { kind: "development", module: loaded.development_module, registry_version: null }
        : { kind: "bundled", version: loaded.version, integrity: loaded.integrity },
      report: await inspectFile(loaded.runtime, parsed.input, requestJson, command === "trace") };
  }
  if (command === "edit") {
    const edit = editArguments(rest);
    const loaded = await loadRuntime(developmentModule);
    const request = edit.operationsJson ?? await readFile(edit.operations, "utf8");
    const options = edit.recalcOptionsJson !== undefined
      ? edit.recalcOptionsJson
      : edit.recalcOptions === undefined
        ? undefined
        : await readFile(edit.recalcOptions, "utf8");
    return {
      mode: "edit",
      runtime: loaded.development_module
        ? { kind: "development", module: loaded.development_module, registry_version: null }
        : { kind: "bundled", version: loaded.version, integrity: loaded.integrity },
      ...await editFile(loaded.runtime, edit.input, request, edit.output, options)
    };
  }
  if (command === "source-recalculate") {
    const source = sourceArguments(rest);
    const loaded = await loadRuntime(developmentModule);
    const options = source.optionsJson !== undefined
      ? source.optionsJson
      : source.options === undefined
        ? undefined
        : await readFile(source.options, "utf8");
    const receipt = await recalculateFile(loaded.runtime, source.input, options, source.output);
    return {
      mode: source.check ? "check" : "recalculate",
      runtime: loaded.development_module
        ? { kind: "development", module: loaded.development_module, registry_version: null }
        : { kind: "bundled", version: loaded.version, integrity: loaded.integrity },
      ...receipt
    };
  }
  if (command !== "doctor" || rest.length) throw new Error(
    "Usage: node scripts/xlsx.mjs doctor | inspect INPUT.xlsx (REQUEST.json | --json JSON | --range Sheet!A1:D20 [--limit N] | --cells Sheet!A1 ... [--limit N]) | trace INPUT.xlsx (REQUEST.json | --json JSON | --roots Sheet!B1 ... [--direction precedents|dependents] [--max-depth N] [--limit N] [--details]) | source-recalculate INPUT.xlsx [--options OPTIONS.json | --options-json JSON] [--output OUTPUT.xlsx | --check] | edit INPUT.xlsx (--operations REQUEST.json | --operations-json JSON) --output OUTPUT.xlsx [--recalc-options OPTIONS.json | --recalc-options-json JSON]"
  );
  const loaded = await loadRuntime(developmentModule);
  return {
    ready: true, node: process.version, runtime: loaded.development_module ? "development" : "bundled",
    version: loaded.version, integrity: loaded.integrity ?? null,
    provenance: loaded.provenance ?? null, wasm: "instantiated",
    capabilities: JSON.parse(loaded.runtime.capabilities())
  };
}

// Hosts may mount the skill through a symlink; Node resolves the module URL.
const invocationPath = process.argv[1] ? await realpath(process.argv[1]).catch(() => null) : null;
if (invocationPath && import.meta.url === pathToFileURL(invocationPath).href) {
  main(process.argv.slice(2)).then(result => console.log(JSON.stringify(result, null, 2))).catch(error => {
    // Canonical errors may be thrown as serialized envelopes; retain their details.
    console.error(typeof error === "string" ? error : error instanceof Error ? error.message : JSON.stringify(error));
    process.exitCode = 1;
  });
}
