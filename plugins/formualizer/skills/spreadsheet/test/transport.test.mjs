import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, writeFile, rm, cp, access, symlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { syntheticWorkbook } from "../scripts/synthetic.mjs";
import { main, loadRuntime } from "../scripts/xlsx.mjs";
const root = fileURLToPath(new URL("../", import.meta.url));
test("doctor verifies bundled hashes and caches a session-free runtime", async () => {
  const report = await main(["doctor"]);
  assert.equal(report.ready, true);
  assert.equal(report.integrity, "verified");
  assert.equal(report.provenance.crates.formualizer, "0.14.0");
  assert.equal(report.capabilities.sessions, false);
  const first = await loadRuntime();
  assert.equal(first.runtime, (await loadRuntime()).runtime);
  assert.equal(first.runtime.createSession, undefined);
});
test("copied skill runs from unrelated cwd without dependencies and rejects tampering", async t => {
  const dir = await mkdtemp(join(tmpdir(), "formualizer-bundle-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const copy = join(dir, "skill");
  await cp(root, copy, { recursive: true });
  await assert.rejects(access(join(copy, "node_modules")));
  await assert.rejects(access(join(copy, "package.json")));
  const cli = join(copy, "scripts/xlsx.mjs");
  const run = () => spawnSync(process.execPath, [cli, "doctor"], { cwd: tmpdir(), encoding: "utf8" });
  assert.equal(run().status, 0);
  const file = join(copy, "engine/formualizer_engine.wasm.br");
  const bytes = await readFile(file);
  bytes[0] ^= 1;
  await writeFile(file, bytes);
  const bad = run();
  assert.equal(bad.status, 1);
  assert.match(bad.stderr, /compressed SHA-256 mismatch/);
});
test("symlinked skill CLI emits doctor JSON and writes a recalculated artifact from unrelated cwd", async t => {
  const dir = await mkdtemp(join(tmpdir(), "formualizer-symlink-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const skill = join(dir, "mounted-skill");
  await symlink(root, skill, "dir");
  const cli = join(skill, "scripts/xlsx.mjs");
  const run = args => spawnSync(process.execPath, [cli, ...args],
    { cwd: tmpdir(), encoding: "utf8", timeout: 30000 });
  const doctor = run(["doctor"]);
  assert.equal(doctor.status, 0, doctor.stderr);
  const report = JSON.parse(doctor.stdout);
  assert.equal(report.ready, true);
  assert.equal(report.version, "0.18.0");
  assert.equal(report.integrity, "verified");

  const input = join(dir, "input.xlsx"), output = join(dir, "output.xlsx");
  await writeFile(input, syntheticWorkbook({
    "xl/worksheets/sheet1.xml": '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><f>21*2</f><v>0</v></c></row></sheetData></worksheet>'
  }));
  const recalc = run(["source-recalculate", input, "--output", output]);
  assert.equal(recalc.status, 0, recalc.stderr);
  const receipt = JSON.parse(recalc.stdout);
  assert.equal(receipt.runtime.version, "0.18.0");
  assert.equal(receipt.runtime.integrity, "verified");
  assert.equal(receipt.metadata.cacheCellsChanged, 1);
  assert.equal(receipt.output, output);
  const bytes = await readFile(output);
  assert.equal(bytes.length, receipt.result_bytes);
  assert.ok(bytes.length > 0);
});

test("shipped entrypoints run through symlinks and stay inert when imported", async t => {
  const dir = await mkdtemp(join(tmpdir(), "formualizer-entrypoints-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const plugin = join(dir, "plugin");
  await symlink(fileURLToPath(new URL("../../../", import.meta.url)), plugin, "dir");
  const server = join(plugin, "server/stdio-mcp.mjs");
  const message = JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize" }) + "\n";
  const result = spawnSync(process.execPath, [server],
    { cwd: tmpdir(), encoding: "utf8", input: message, timeout: 30000 });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(JSON.parse(result.stdout).result.serverInfo.name, "formualizer-spreadsheet");

  const imports = `await import(${JSON.stringify(new URL("../scripts/xlsx.mjs", import.meta.url).href)});
await import(${JSON.stringify(new URL("../../../server/stdio-mcp.mjs", import.meta.url).href)});
console.log(JSON.stringify({dataListeners: process.stdin.listenerCount("data")}));`;
  // A different file with the server's basename must not trigger its CLI guard.
  const runner = join(dir, "stdio-mcp.mjs");
  await writeFile(runner, imports);
  for (const args of [[runner], ["--input-type=module", "-e", imports]]) {
    const imported = spawnSync(process.execPath, args,
      { cwd: tmpdir(), encoding: "utf8", input: message, timeout: 30000 });
    assert.equal(imported.status, 0, imported.stderr);
    assert.equal(imported.stdout, '{"dataListeners":0}\n');
    assert.equal(imported.stderr, "");
  }
});

test("CLI rejects removed session commands", () => {
  for (const command of ["run", "discover", "probe"]) {
    const result = spawnSync(process.execPath, [join(root, "scripts/xlsx.mjs"), command], { encoding: "utf8" });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /Usage:/);
  }
});
