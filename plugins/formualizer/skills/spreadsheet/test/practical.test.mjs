import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { loadRuntime, recalculateFile } from "../scripts/xlsx.mjs";
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
const real = (name, fn) => test(name, fn);
const sheet = "xl/worksheets/sheet1.xml";
const input = () => syntheticWorkbook({
  [sheet]: '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><v>21</v></c><c r="B1"><f>A1*2</f><v>0</v></c></row></sheetData></worksheet>'
});
real("matching runtime returns edited iterative artifact with deferred receipt", async () => {
  const { runtime } = await loadRuntime(selected);
  const request = JSON.stringify({recalculate:"always",ops:[{
    kind:"set_cells",sheet_name:"Sheet1",overwrite_formulas:true,
    cells:{A1:{kind:"value",value:30},B1:{kind:"formula",formula:"(B1+1)/2"}}
  }]});
  const options = JSON.stringify({calculation:{iteration:{mode:"iterate",maxIterations:10}}});
  const result = runtime.editXlsx(new Uint8Array(input()), request, options);
  try {
    const meta = JSON.parse(result.metadataJson);
    assert.equal(meta.edit.calculation.cache_state, "deferred");
    assert.equal(meta.edit.calculation.attempted, true);
    assert.equal(meta.edit.calculation.completed, false);
    assert.equal(meta.recalculation.coverage, "deferred");
    assert.equal(meta.recalculation.formulaCells, null);
    assert.equal(meta.recalculation.calculationFlagsChanged, null);
    assert.equal(meta.recalculation.deferredReason, "iterative_initial_state_unverified");
    const read = runtime.inspectXlsx(result.bytes,
      JSON.stringify({selection:{kind:"cells",addresses:["Sheet1!A1"]}}));
    try { assert.equal(JSON.parse(read.metadataJson).items[0].source_value.value, 30); }
    finally { read.free(); }
  } finally { result.free(); }
  for (const extra of [{profile:"strict"},{requireComplete:true},{limits:{maxOutputBytes:1}}]) {
    assert.throws(() => runtime.editXlsx(new Uint8Array(input()), request,
      JSON.stringify({...extra,calculation:{iteration:{mode:"iterate"}}})));
  }
});

real("matching source runtime carries partial diagnostics and invalidates dependent caches", async () => {
  const { runtime } = await loadRuntime(selected);
  const bytes = new Uint8Array(syntheticWorkbook({
    [sheet]: '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><f>UNAVAILABLE_FN(1)</f><v>123</v></c><c r="B1"><f>A1+1</f><v>124</v></c><c r="C1"><f>2+2</f><v>0</v></c></row></sheetData></worksheet>'
  }));
  const result = runtime.recalculateSourceXlsx(bytes, '{"errorLocationLimit":0}');
  try {
    const meta = JSON.parse(result.metadataJson);
    assert.equal(meta.coverage, "partial");
    assert.equal(meta.coverageScope, "conservative");
    assert.equal(meta.cachesInvalidated, 2);
    assert.equal(meta.operationDiagnostics.logicalRootCellCount, 1);
    assert.equal(meta.calculationFlagsChanged, true);
    assert.equal(meta.operationDiagnostics.unavailableFunctions.UNAVAILABLE_FN.cellReasonCount, 1);
    assert.equal(meta.operationDiagnostics.unavailableResultCells, 0);
    assert.deepEqual(meta.operationDiagnostics.workbookFeatures, []);
    assert.deepEqual(meta.operationDiagnostics.samples, []);
    assert.ok(meta.operationDiagnostics.samplesTruncated > 0);
    assert.equal(meta.completed, true);
    const read = runtime.inspectXlsx(result.bytes,
      JSON.stringify({selection:{kind:"cells",addresses:["Sheet1!A1","Sheet1!B1","Sheet1!C1"]}}));
    try {
      const items = JSON.parse(read.metadataJson).items;
      assert.equal(items[0].stored_formula_cache?.value ?? null, null);
      assert.equal(items[1].stored_formula_cache?.value ?? null, null);
      assert.equal(items[2].stored_formula_cache.value.value, 4);
    } finally { read.free(); }
  } finally { result.free(); }
  assert.throws(() => runtime.recalculateSourceXlsx(bytes, '{"requireComplete":true}'));
});

real("matching runtime preserves precision deferral and strict shared workbook feature", async () => {
  const { runtime } = await loadRuntime(selected);
  const bytes = new Uint8Array(syntheticWorkbook({
    "xl/workbook.xml": '<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets><calcPr fullPrecision="0"/></workbook>'
  }));
  const result = runtime.recalculateSourceXlsx(bytes);
  try {
    const meta = JSON.parse(result.metadataJson);
    assert.deepEqual(result.bytes, bytes);
    assert.equal(meta.coverage, "deferred");
    assert.equal(meta.deferredReason, "workbook_precision_unverified");
    assert.equal(meta.calculationFlagsChanged, null);
    assert.equal(meta.completed, false);
    assert.equal(meta.bytesUnchanged, true);
  } finally { result.free(); }
  const strict = runtime.recalculateSourceXlsx(bytes, '{"profile":"strict"}');
  try {
    const meta = JSON.parse(strict.metadataJson);
    assert.equal(meta.coverage, "partial");
    assert.deepEqual(meta.operationDiagnostics.workbookFeatures, ["precision_as_displayed"]);
    assert.equal(typeof meta.calculationFlagsChanged, "boolean");
  } finally { strict.free(); }
  assert.throws(() => runtime.recalculateSourceXlsx(bytes, '{"requireComplete":true}'));
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
    assert.equal(meta.calculationFlagsChanged, false);
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
      assert.equal(meta.recalculation, undefined);
    } finally { result.free(); }
    for (const calculation of [{maxElapsedMs:0},{iteration:{mode:"iterate",maxIterations:0}},
      {workbookSeed:9007199254740992},{unknown:1}]) {
      assert.throws(() => runtime.editXlsx(new Uint8Array(input()), editRequest(mode),
        JSON.stringify({ calculation })), error => error.code === "INVALID_ARGUMENT");
    }
  }
});
