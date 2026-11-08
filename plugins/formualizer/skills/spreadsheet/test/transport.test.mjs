import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, writeFile, rm, cp, access } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { loadRuntime, executeFile, main } from "../scripts/run.mjs";
import { syntheticWorkbook, probeRequest } from "../scripts/synthetic.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const cli = join(root, "scripts/run.mjs");

async function temporary(t) {
  const directory = await mkdtemp(join(tmpdir(), "formualizer-transport-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  return directory;
}

function values(response) {
  const dense = response.data.blocks[0].payload.dense;
  return dense.row_runs[0].flatMap(run => Array(run.len).fill(dense.dictionary[run.value_idx]));
}

test("runtime metadata does not impose an unnecessary Node24 minimum", async () => {
  const pkg = JSON.parse(await readFile(join(root, "package.json"), "utf8"));
  const lock = JSON.parse(await readFile(join(root, "package-lock.json"), "utf8"));
  assert.equal(pkg.engines.node, ">=20");
  assert.equal(lock.packages[""].engines.node, ">=20");
});

test("doctor instantiates the exact pinned WASM and discovery stays canonical", async () => {
  const report = await main(["doctor"]);
  assert.equal(report.ready, true);
  assert.equal(report.version, "0.17.0");
  assert.equal(report.source_recalculation, true);
  assert.equal(report.stateless_editing, true);
  assert.equal(report.stateless_inspection, true);
  assert.equal(report.engine_navigation, true);
  const { runtime } = await loadRuntime();
  assert.deepEqual(await main(["discover"]), JSON.parse(runtime.operations()));
  assert.deepEqual(await main(["discover", "read_cells"]),
    JSON.parse(runtime.operations()).find(item => item.name === "read_cells"));
});

test("real bounded read and export/reopen preserve source bytes", async t => {
  const directory = await temporary(t);
  const input = join(directory, "source.xlsx"), output = join(directory, "export.xlsx");
  const source = syntheticWorkbook();
  await writeFile(input, source);
  const { runtime } = await loadRuntime();
  const response = await executeFile(runtime, input, probeRequest, output);
  assert.equal(response.operation, "read_cells");
  assert.equal(response.data.page.cells_returned, 2);
  assert.equal(response.data.page.complete, true);
  assert.equal(response.data.calculation.state, "not_evaluated");
  assert.deepEqual(values(response), [21, 42]);
  assert.deepEqual(await readFile(input), source);
  assert.deepEqual([...((await readFile(output)).subarray(0, 2))], [0x50, 0x4b]);
  assert.deepEqual(values(await executeFile(runtime, output, probeRequest)), [21, 42]);
});

test("only explicit output overwrites its ordinary filesystem destination", async t => {
  const directory = await temporary(t);
  const input = join(directory, "source.xlsx"), output = join(directory, "out.xlsx");
  const source = syntheticWorkbook();
  await writeFile(input, source);
  const { runtime } = await loadRuntime();
  await executeFile(runtime, input, probeRequest);
  assert.deepEqual(await readFile(input), source);
  await assert.rejects(access(output));
  await writeFile(output, "previous destination content");
  await executeFile(runtime, input, probeRequest, output);
  assert.deepEqual(values(await executeFile(runtime, output, probeRequest)), [21, 42]);
  assert.deepEqual(await readFile(input), source);
  // An explicitly selected source destination is also serialized and overwritten.
  await executeFile(runtime, input, probeRequest, input);
  assert.notDeepEqual(await readFile(input), source);
  assert.deepEqual(values(await executeFile(runtime, input, probeRequest)), [21, 42]);
});

test("canonical rejection leaves destination untouched and disposes session", async t => {
  const directory = await temporary(t);
  const input = join(directory, "source.xlsx"), output = join(directory, "output.xlsx");
  await writeFile(input, syntheticWorkbook());
  await writeFile(output, "keep");
  const { runtime } = await loadRuntime();
  let session;
  const tracked = {
    createSession(bytes) { session = runtime.createSession(bytes); return session; },
    executeOperation: (...args) => runtime.executeOperation(...args),
    exportWorkbook: (...args) => runtime.exportWorkbook(...args),
    disposeSession: (...args) => runtime.disposeSession(...args)
  };
  await assert.rejects(executeFile(tracked, input, { operation: "read_cells", params: {} }, output),
    error => { assert.equal(error.error.code, "INVALID_REQUEST"); return true; });
  assert.equal(await readFile(output, "utf8"), "keep");
  assert.equal(runtime.disposeSession(session), false);
});

test("synthetic host probe produces checkable receipts and refuses an existing directory", async t => {
  const directory = await temporary(t), destination = join(directory, "probe");
  const receipt = await main(["probe", destination]);
  assert.deepEqual(values(receipt.response), [21, 42]);
  assert.deepEqual(values(receipt.reopened), [21, 42]);
  assert.deepEqual(JSON.parse(await readFile(join(destination, "receipt.json"), "utf8")), receipt);
  await assert.rejects(main(["probe", destination]), /EEXIST/);
});

test("CLI errors are actionable and never report success", async t => {
  const directory = await temporary(t);
  const request = join(directory, "bad.json");
  await writeFile(request, "{");
  for (const [args, message] of [
    [[], /Usage:/],
    [["run", "missing.xlsx", request], /Cannot read\/parse request JSON/],
    [["discover", "not_an_operation"], /Operation absent/]
  ]) {
    const result = spawnSync(process.execPath, [cli, ...args], { encoding: "utf8" });
    assert.equal(result.status, 1);
    assert.match(result.stderr, message);
    assert.equal(result.stdout, "");
  }
  const input = join(directory, "source.xlsx"), invalid = join(directory, "invalid.json");
  await writeFile(input, syntheticWorkbook());
  await writeFile(invalid, JSON.stringify({ operation: "read_cells", params: {} }));
  const canonical = spawnSync(process.execPath, [cli, "run", input, invalid], { encoding: "utf8" });
  assert.equal(canonical.status, 1);
  assert.equal(canonical.stdout, "");
  assert.equal(JSON.parse(canonical.stderr).error.code, "INVALID_REQUEST");
  const { runtime } = await loadRuntime();
  await assert.rejects(executeFile(runtime, "unused", { operation: "read_cells", params: [] }), /Request must be/);
});

test("copied skill resolves local assets from unrelated cwd; absent dependency never auto-installs", async t => {
  const directory = await temporary(t), copied = join(directory, "copied skill");
  for (const part of ["scripts", "package.json", "package-lock.json"]) {
    await cp(join(root, part), join(copied, part), { recursive: true });
  }
  const copiedCli = join(copied, "scripts/run.mjs");
  const missing = spawnSync(process.execPath, [copiedCli, "doctor"], { cwd: tmpdir(), encoding: "utf8" });
  assert.equal(missing.status, 1);
  assert.match(missing.stderr, /npm ci --ignore-scripts/);
  await assert.rejects(access(join(copied, "node_modules")));
  await cp(join(root, "node_modules"), join(copied, "node_modules"), { recursive: true });
  const ready = spawnSync(process.execPath, [copiedCli, "probe", join(directory, "copied-probe")],
    { cwd: tmpdir(), encoding: "utf8" });
  assert.equal(ready.status, 0, ready.stderr);
  assert.equal(JSON.parse(ready.stdout).package_version, "0.17.0");
});

test("directory-compatible manifest and skill resources are present", async () => {
  const manifest = JSON.parse(await readFile(new URL("../../../.claude-plugin/plugin.json", import.meta.url), "utf8"));
  assert.equal(manifest.name, "formualizer");
  assert.match(manifest.version, /^0\.17\.0$/);
  assert.equal(manifest.license, "Apache-2.0");
  assert.ok(manifest.description.length > 20);
  const skill = await readFile(join(root, "SKILL.md"), "utf8");
  assert.match(skill, /^---\nname: spreadsheet\ndescription: .+\n---/);
  for (const part of ["scripts/run.mjs", "scripts/synthetic.mjs", "references/host-probe.md", "package-lock.json"]) {
    await access(join(root, part));
  }
});
