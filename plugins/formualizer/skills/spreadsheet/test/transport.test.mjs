import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, writeFile, rm, cp, access } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
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
test("CLI rejects removed session commands", () => {
  for (const command of ["run", "discover", "probe"]) {
    const result = spawnSync(process.execPath, [join(root, "scripts/xlsx.mjs"), command], { encoding: "utf8" });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /Usage:/);
  }
});
