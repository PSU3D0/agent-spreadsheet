import { readFile, writeFile, mkdir } from "node:fs/promises";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import { resolve, join, sep } from "node:path";
import { syntheticWorkbook, probeRequest } from "./synthetic.mjs";
import { createHash } from "node:crypto";

const require = createRequire(import.meta.url);
const expectedVersion = "0.17.0";
const expectedIntegrity = "sha512-LpmEt1GNA91zvalLKJ4/J7VKe+gMpneBsoe8ZukQKwCSulIh7DAoXuiVwrEjCPoVaTRwcwlZa8407zFsSoYtag==";
const skillRoot = fileURLToPath(new URL("../", import.meta.url));
const setup = `Run explicitly in the skill folder (${skillRoot}): npm ci --ignore-scripts. Requires Node >=20 and npm; keep host install/network protections enabled.`;

export async function loadRuntime(developmentModule) {
  if (Number(process.versions.node.split(".")[0]) < 20) throw new Error("Node >=20 required for the runtime's WebCrypto-backed sessions. Select a supported Node version using your host's normal setup.");
  if (developmentModule !== undefined) {
    if (typeof developmentModule !== "string" || !developmentModule ||
        /^[a-z][a-z0-9+.-]*:/i.test(developmentModule)) {
      throw new Error("--dev-runtime requires an explicit local loader path, not a URL.");
    }
    const local = resolve(developmentModule);
    const loader = await import(pathToFileURL(local).href);
    if (typeof loader.createWasmRuntime !== "function") throw new Error("Development loader must export createWasmRuntime().");
    return { runtime: await loader.createWasmRuntime(), version: null, development_module: local };
  }
  let pkg;
  try {
    const location = require.resolve("agent-spreadsheet-wasm/package.json");
    if (!location.startsWith(join(skillRoot, "node_modules") + sep)) throw new Error("Dependency must be installed inside this skill");
    pkg = require(location);
  }
  catch { throw new Error("Missing skill-local agent-spreadsheet-wasm dependency. " + setup); }
  if (pkg.version !== expectedVersion) throw new Error(`Expected agent-spreadsheet-wasm ${expectedVersion}, found ${pkg.version}. ${setup}`);
  const lock = JSON.parse(await readFile(new URL("../package-lock.json", import.meta.url), "utf8"));
  const pinned = lock.packages["node_modules/agent-spreadsheet-wasm"];
  if (pinned?.version !== expectedVersion || pinned?.integrity !== expectedIntegrity) throw new Error("Dependency lock identity differs from the approved 0.17.0 pin.");
  const { createWasmRuntime } = await import("agent-spreadsheet-wasm");
  const runtime = await createWasmRuntime();
  for (const name of ["createSession", "operations", "executeOperation", "exportWorkbook", "disposeSession", "recalculateSourceXlsx", "editXlsx", "inspectXlsx", "traceXlsx"]) {
    if (typeof runtime[name] !== "function") throw new Error("WASM missing binding: " + name);
  }
  return { runtime, version: pkg.version };
}

export async function executeFile(runtime, input, request, output) {
  if (!request || typeof request !== "object" || Array.isArray(request) ||
      typeof request.operation !== "string" || !request.operation ||
      !request.params || typeof request.params !== "object" || Array.isArray(request.params)) {
    throw new Error('Request must be {"operation":"canonical_name","params":{...}}. Params are validated by the canonical runtime.');
  }
  const id = await runtime.createSession(new Uint8Array(await readFile(input)));
  try {
    const raw = await runtime.executeOperation(id, request.operation, JSON.stringify(request.params));
    const response = JSON.parse(raw);
    if (output !== undefined) await writeFile(output, await runtime.exportWorkbook(id));
    return response;
  } finally {
    await runtime.disposeSession(id);
  }
}

const digest = bytes => createHash("sha256").update(bytes).digest("hex");

// Uses original bytes directly. Workbook semantics and options belong entirely
// to the binding; this transport only owns file I/O and result lifetime.
export async function recalculateFile(runtime, input, optionsJson, output) {
  if (typeof runtime.recalculateSourceXlsx !== "function") {
    throw new Error("Runtime does not export recalculateSourceXlsx. Registry 0.16.0 lacks this binding; select --dev-runtime LOCAL_LOADER.mjs built with matching source glue/WASM. No session fallback is available.");
  }
  const receipt = await byteFile(input, output, bytes => runtime.recalculateSourceXlsx(bytes, optionsJson));
  // Legacy current means byte identity only; it makes no freshness claim.
  return { ...receipt, bytesUnchanged: receipt.bytes_unchanged, current: receipt.bytes_unchanged };
}

export async function editFile(runtime, input, requestJson, output, recalcOptionsJson) {
  if (typeof runtime.editXlsx !== "function") {
    throw new Error("Runtime does not export editXlsx. Registry 0.16.0 lacks this binding; select --dev-runtime LOCAL_LOADER.mjs built with matching source glue/WASM. No resident-session fallback is available.");
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
        : { kind: "registry", version: loaded.version, lock_integrity: expectedIntegrity },
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
        : { kind: "registry", version: loaded.version, lock_integrity: expectedIntegrity },
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
        : { kind: "registry", version: loaded.version, lock_integrity: expectedIntegrity },
      ...receipt
    };
  }
  const arities = { doctor: [0], discover: [0, 1], run: [2, 3], probe: [1] };
  if (!arities[command]?.includes(rest.length)) throw new Error(
    "Usage: node scripts/run.mjs [--dev-runtime LOCAL_LOADER.mjs] doctor | discover [operation] | inspect INPUT.xlsx REQUEST.json | trace INPUT.xlsx REQUEST.json | run INPUT.xlsx REQUEST.json [OUTPUT.xlsx] | source-recalculate INPUT.xlsx [--options OPTIONS.json] [--output OUTPUT.xlsx | --check] | edit INPUT.xlsx --operations REQUEST.json --output OUTPUT.xlsx [--recalc-options OPTIONS.json] | probe NEW_DIRECTORY"
  );
  const { runtime, version, development_module } = await loadRuntime(developmentModule);
  if (command === "doctor" && development_module) return {
    ready: true, node: process.version, runtime: "development", module: development_module,
    registry_version: null, lock_integrity: null, wasm: "instantiated",
    source_recalculation: typeof runtime.recalculateSourceXlsx === "function",
    stateless_editing: typeof runtime.editXlsx === "function",
    stateless_inspection: typeof runtime.inspectXlsx === "function",
    engine_navigation: typeof runtime.traceXlsx === "function",
    scope: "explicit local development runtime; not the pinned registry package or host certification"
  };
  if (command === "doctor") return {
    ready: true, node: process.version, package: "agent-spreadsheet-wasm", version,
    lock_integrity: expectedIntegrity, wasm: "instantiated", skill_directory: skillRoot,
    source_recalculation: typeof runtime.recalculateSourceXlsx === "function",
    stateless_editing: typeof runtime.editXlsx === "function",
    stateless_inspection: typeof runtime.inspectXlsx === "function",
    engine_navigation: typeof runtime.traceXlsx === "function",
    scope: "pinned registry 0.17.0 runtime with stateless source recalculation, batch editing, inspection, and dependency tracing"
  };
  if (command === "discover") {
    const discovery = JSON.parse(await runtime.operations());
    if (!rest.length) return discovery;
    const operation = discovery.find(item => item.name === rest[0]);
    if (!operation) throw new Error("Operation absent from this runtime: " + rest[0]);
    return operation;
  }
  if (command === "run") {
    let request;
    try { request = JSON.parse(await readFile(rest[1], "utf8")); }
    catch (error) { throw new Error("Cannot read/parse request JSON: " + error.message); }
    return executeFile(runtime, rest[0], request, rest[2]);
  }
  const directory = resolve(rest[0]);
  await mkdir(directory); // Deliberately requires a new directory so probe files cannot clobber input.
  const input = join(directory, "input.xlsx"), output = join(directory, "export.xlsx");
  await writeFile(input, syntheticWorkbook());
  await writeFile(join(directory, "request.json"), JSON.stringify(probeRequest, null, 2) + "\n");
  const response = await executeFile(runtime, input, probeRequest, output);
  const reopened = await executeFile(runtime, output, probeRequest);
  const dense = response.data?.blocks?.[0]?.payload?.dense;
  const reread = reopened.data?.blocks?.[0]?.payload?.dense;
  if (JSON.stringify(dense) !== JSON.stringify(reread) ||
      JSON.stringify(dense?.dictionary) !== JSON.stringify([null, 21, 42]) ||
      response.data?.page?.cells_returned !== 2) {
    throw new Error("Synthetic read/export probe did not return the expected bounded values; inspect the runtime response.");
  }
  const receipt = {
    scope: `${development_module ? "development runtime" : "0.17.0"} transport feasibility only; B1=42 is a supplied cache, not proof of recalculation or Formualizer 0.14 source preservation`,
    node: process.version, package_version: version, input, output, response, reopened
  };
  await writeFile(join(directory, "receipt.json"), JSON.stringify(receipt, null, 2) + "\n");
  return receipt;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main(process.argv.slice(2)).then(result => console.log(JSON.stringify(result, null, 2))).catch(error => {
    // Canonical errors may be thrown as serialized envelopes; retain their details.
    console.error(typeof error === "string" ? error : error instanceof Error ? error.message : JSON.stringify(error));
    process.exitCode = 1;
  });
}
