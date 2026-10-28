import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { loadRuntime, recalculateFile } from "../scripts/run.mjs";
import { syntheticWorkbook } from "../scripts/synthetic.mjs";

test("byte equality stays separate from deferred coverage and legacy current", async t => {
  const dir = await mkdtemp(join(tmpdir(), "practical-recalc-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const input = join(dir, "in.xlsx"), bytes = Buffer.from("valid binding-owned bytes");
  await writeFile(input, bytes);
  let freed = 0;
  const receipt = await recalculateFile({
    recalculateSourceXlsx() {
      return { bytes: new Uint8Array(bytes),
        metadataJson: JSON.stringify({ coverage: "deferred", completed: false, formulaCells: null }),
        free() { freed++; } };
    }
  }, input);
  assert.equal(receipt.bytes_unchanged, true);
  assert.equal(receipt.bytesUnchanged, true);
  assert.equal(receipt.current, true);
  assert.equal(receipt.metadata.coverage, "deferred");
  assert.equal(receipt.metadata.completed, false);
  assert.equal(receipt.metadata.formulaCells, null);
  assert.equal(freed, 1);
});

test("source option/error failure cannot publish a destination", async t => {
  const dir = await mkdtemp(join(tmpdir(), "practical-output-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const source = join(dir, "in.xlsx"), destination = join(dir, "out.xlsx");
  await writeFile(source, "input bytes");
  await writeFile(destination, "existing destination");
  await assert.rejects(recalculateFile({
    recalculateSourceXlsx() { throw new Error("resource or options failure"); }
  }, source, "{}", destination), /resource or options failure/);
  const { readFile } = await import("node:fs/promises");
  assert.equal(await readFile(destination, "utf8"), "existing destination");
});

const selected = process.env.FORMUALIZER_TEST_RUNTIME;
const real = (name, fn) => test(name, { skip: !selected && "Select matching practical source runtime" }, fn);
const sheet = "xl/worksheets/sheet1.xml";
const input = () => syntheticWorkbook({
  [sheet]: '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><v>21</v></c><c r="B1"><f>A1*2</f><v>0</v></c></row></sheetData></worksheet>'
});
const editRequest = mode => JSON.stringify({ recalculate: mode, ops: [
  {kind:"style",sheet_name:"Sheet1",target:{kind:"range",range:"A1"},patch:{font:{bold:true}}}
] });

real("matching runtime preserves exact shared seed and complete coverage", async () => {
  const { runtime } = await loadRuntime(selected);
  const result = runtime.recalculateSourceXlsx(new Uint8Array(input()), JSON.stringify({
    calculation: { workbookSeed:"18446744073709551615",fixedNowUtc:"2025-01-02T03:04:05Z",timezone:"utc" }
  }));
  try {
    const meta = JSON.parse(result.metadataJson);
    assert.equal(meta.calculation.workbookSeed, "18446744073709551615");
    assert.equal(meta.coverage, "complete");
    assert.equal(meta.attempted, true);
    assert.equal(meta.completed, true);
    assert.equal(meta.artifactAvailable, true);
    assert.equal(meta.bytesUnchanged, false);
    assert.equal(meta.cachesInvalidated, 0);
    assert.equal(typeof meta.operationDiagnostics, "object");
  } finally { result.free(); }
});

real("matching edit runtime validates calculation in never and preserving auto", async () => {
  const { runtime } = await loadRuntime(selected);
  for (const mode of ["never","auto"]) {
    const result = runtime.editXlsx(new Uint8Array(input()), editRequest(mode),
      '{"calculation":{"workbookSeed":"18446744073709551615"}}');
    try {
      const meta = JSON.parse(result.metadataJson);
      assert.equal(meta.edit.calculation.attempted, false);
      assert.equal(meta.edit.calculation.completed, false);
      assert.equal(meta.edit.calculation.cache_state, "unknown");
    } finally { result.free(); }
    for (const calculation of [{maxElapsedMs:0},{iteration:{mode:"iterate",maxIterations:0}},
      {workbookSeed:9007199254740992},{unknown:1}]) {
      assert.throws(() => runtime.editXlsx(new Uint8Array(input()), editRequest(mode),
        JSON.stringify({ calculation })), error => error.code === "INVALID_ARGUMENT");
    }
  }
});
