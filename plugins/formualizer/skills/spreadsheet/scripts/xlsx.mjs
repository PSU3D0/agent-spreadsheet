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
      if (options[key] !== undefined) throw new Error("Duplicate source argument: " + flag);
      options[key] = flags[++index];
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
    const key = { "--operations": "operations", "--output": "output", "--recalc-options": "recalcOptions" }[flags[index]];
    if (!key || parsed[key] !== undefined || !flags[index + 1] || flags[index + 1].startsWith("--")) {
      throw new Error("Invalid edit argument: " + flags[index]);
    }
    parsed[key] = flags[++index];
  }
  if (!parsed.operations || !parsed.output) throw new Error("edit requires --operations REQUEST.json and --output OUTPUT.xlsx.");
  return parsed;
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
    if (rest.length !== 2 || rest.some(arg => arg.startsWith("--"))) throw new Error(command + " requires INPUT.xlsx REQUEST.json.");
    const loaded = await loadRuntime(developmentModule);
    return { mode: command,
      runtime: loaded.development_module
        ? { kind: "development", module: loaded.development_module, registry_version: null }
        : { kind: "bundled", version: loaded.version, integrity: loaded.integrity },
      report: await inspectFile(loaded.runtime, rest[0], await readFile(rest[1], "utf8"), command === "trace") };
  }
  if (command === "edit") {
    const edit = editArguments(rest);
    const loaded = await loadRuntime(developmentModule);
    const request = await readFile(edit.operations, "utf8");
    const options = edit.recalcOptions === undefined ? undefined : await readFile(edit.recalcOptions, "utf8");
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
    const options = source.options === undefined ? undefined : await readFile(source.options, "utf8");
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
    "Usage: node scripts/xlsx.mjs doctor | inspect INPUT.xlsx REQUEST.json | trace INPUT.xlsx REQUEST.json | source-recalculate INPUT.xlsx [--options OPTIONS.json] [--output OUTPUT.xlsx | --check] | edit INPUT.xlsx --operations REQUEST.json --output OUTPUT.xlsx [--recalc-options OPTIONS.json]"
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
