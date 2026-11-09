import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, writeFile, rm, access } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { inflateRawSync } from "node:zlib";
import { loadRuntime, recalculateFile, editFile, main } from "../scripts/xlsx.mjs";
import { syntheticWorkbook } from "../scripts/synthetic.mjs";

const cli = fileURLToPath(new URL("../scripts/xlsx.mjs", import.meta.url));
// Test-only selection. Production selection is always an explicit CLI argument.
const developmentModule = process.env.FORMUALIZER_TEST_RUNTIME;
const sourceTest = (name, fn) => test(name, fn);
const sheetName = "xl/worksheets/sheet1.xml";
const worksheet = rows => `<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>${rows}</sheetData></worksheet>`;
const stale = () => syntheticWorkbook({
  [sheetName]: worksheet('<row r="1"><c r="A1"><v>21</v></c><c r="B1"><f>A1*2</f><v>0</v></c></row>'),
  "custom/opaque.xml": "<opaque>preserve this source member</opaque>"
});

async function temporary(t) {
  const directory = await mkdtemp(join(tmpdir(), "formualizer-source-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  return directory;
}

// Read a member for assertions only; never evaluate or rewrite workbook data.
function member(bytes, name) {
  let end = bytes.length - 22;
  while (end >= 0 && bytes.readUInt32LE(end) !== 0x06054b50) end--;
  assert.ok(end >= 0);
  const count = bytes.readUInt16LE(end + 10);
  let offset = bytes.readUInt32LE(end + 16);
  for (let index = 0; index < count; index++) {
    assert.equal(bytes.readUInt32LE(offset), 0x02014b50);
    const method = bytes.readUInt16LE(offset + 10);
    const size = bytes.readUInt32LE(offset + 20);
    const nameLength = bytes.readUInt16LE(offset + 28);
    const extraLength = bytes.readUInt16LE(offset + 30);
    const commentLength = bytes.readUInt16LE(offset + 32);
    const filename = bytes.subarray(offset + 46, offset + 46 + nameLength).toString();
    if (filename === name) {
      const local = bytes.readUInt32LE(offset + 42);
      const start = local + 30 + bytes.readUInt16LE(local + 26) + bytes.readUInt16LE(local + 28);
      const content = bytes.subarray(start, start + size);
      assert.ok(method === 0 || method === 8);
      return (method === 0 ? content : inflateRawSync(content)).toString();
    }
    offset += 46 + nameLength + extraLength + commentLength;
  }
  assert.fail("ZIP member absent: " + name);
}

function runSource(args) {
  const prefix = developmentModule ? ["--dev-runtime", developmentModule] : [];
  return spawnSync(process.execPath, [cli, ...prefix,
    "source-recalculate", ...args], { encoding: "utf8" });
}
function success(result) {
  assert.equal(result.status, 0, result.stderr);
  return JSON.parse(result.stdout);
}

test("default bundled runtime 0.18.0 exposes recalculateSourceXlsx while rejecting missing binding without a session fallback", async t => {
  const directory = await temporary(t), input = join(directory, "input.xlsx");
  await writeFile(input, stale());
  const pinned = await loadRuntime();
  assert.equal(pinned.version, "0.18.0");
  assert.equal(typeof pinned.runtime.recalculateSourceXlsx, "function");
  await assert.rejects(recalculateFile({}, input), /does not export recalculateSourceXlsx/);
  assert.deepEqual(await readFile(input), stale());
});

test("development loader accepts local paths only", async () => {
  await assert.rejects(loadRuntime("https://example.invalid/runtime.mjs"), /local loader path, not a URL/);
  await assert.rejects(loadRuntime("file:///tmp/runtime.mjs"), /local loader path, not a URL/);
});

test("source transport never creates a session and always frees returned results", async t => {
  const directory = await temporary(t), input = join(directory, "input.xlsx");
  await writeFile(input, stale());
  for (const badMetadata of [false, true]) {
    let freed = 0;
    const runtime = {
      createSession() { assert.fail("source path called createSession"); },
      recalculateSourceXlsx() {
        return { bytes: new Uint8Array([1, 2]), metadataJson: badMetadata ? "{" : "{}",
          free() { freed++; } };
      }
    };
    // Successful decoding followed by a filesystem error also releases the result.
    await assert.rejects(recalculateFile(runtime, input, undefined,
      badMetadata ? undefined : join(directory, "missing", "output.xlsx")));
    assert.equal(freed, 1);
  }
  await assert.rejects(main(["source-recalculate", input, "--check", "--output", input]),
    /cannot be combined/);
});

sourceTest("generated source runtime returns typed bytes and metadata; direct repair becomes exact no-op", async t => {
  const directory = await temporary(t), input = join(directory, "input.xlsx");
  const original = stale();
  await writeFile(input, original);
  const { runtime, version } = await loadRuntime(developmentModule);
  assert.equal(version, developmentModule ? null : "0.18.0");
  let result;
  try {
    result = runtime.recalculateSourceXlsx(new Uint8Array(original), "{}");
    assert.ok(result.bytes instanceof Uint8Array);
    assert.equal(typeof result.metadataJson, "string");
    assert.equal(JSON.parse(result.metadataJson).cacheCellsChanged, 1);
    const repaired = Buffer.from(result.bytes);
    assert.match(member(repaired, sheetName), /<f>A1\*2<\/f><v>42<\/v>/);
    assert.equal(member(repaired, "custom/opaque.xml"), member(original, "custom/opaque.xml"));
    let noop;
    try {
      noop = runtime.recalculateSourceXlsx(new Uint8Array(repaired));
      assert.deepEqual(Buffer.from(noop.bytes), repaired);
      assert.equal(JSON.parse(noop.metadataJson).cacheCellsChanged, 0);
    } finally { noop?.free(); }
  } finally { result?.free(); }
  let calls = 0, freed = 0;
  const receipt = await recalculateFile({
    createSession() { assert.fail("source path created a session"); },
    recalculateSourceXlsx(...args) {
      calls++;
      const actual = runtime.recalculateSourceXlsx(...args);
      return { get bytes() { return actual.bytes; }, get metadataJson() { return actual.metadataJson; },
        free() { freed++; actual.free(); } };
    }
  }, input);
  assert.equal(calls, 1);
  assert.equal(freed, 1);
  assert.equal(receipt.current, false);
  assert.deepEqual(await readFile(input), original);
});

sourceTest("same CLI check and explicit output produce truthful receipts without implicit input writes", async t => {
  const directory = await temporary(t), input = join(directory, "input.xlsx"), output = join(directory, "out.xlsx");
  const original = stale();
  await writeFile(input, original);
  const checked = success(runSource([input, "--check"]));
  assert.equal(checked.mode, "check");
  assert.equal(checked.current, false);
  assert.equal(checked.output, null);
  assert.equal(checked.runtime.kind, developmentModule ? "development" : "bundled");
  if (developmentModule) assert.equal(checked.runtime.registry_version, null);
  else assert.equal(checked.runtime.version, "0.18.0");
  await assert.rejects(access(output));
  assert.deepEqual(await readFile(input), original);
  const calculated = success(runSource([input]));
  assert.equal(calculated.output, null);
  assert.deepEqual(await readFile(input), original);
  const written = success(runSource([input, "--output", output]));
  assert.equal(written.output, output);
  assert.equal(written.metadata.summary.evaluated, 1);
  assert.equal(written.metadata.summary.errors, 0);
  assert.equal(written.metadata.cacheCellsChanged, 1);
  assert.match(member(await readFile(output), sheetName), /<v>42<\/v>/);
  const noop = success(runSource([output, "--check"]));
  assert.equal(noop.current, true);
  assert.equal(noop.result_sha256, written.result_sha256);
  assert.deepEqual(await readFile(input), original);
});

sourceTest("real source failures never overwrite an existing destination", async t => {
  const directory = await temporary(t), input = join(directory, "input.xlsx"),
    output = join(directory, "out.xlsx"), options = join(directory, "options.json");
  const malformedFormula = syntheticWorkbook({ [sheetName]: worksheet(
    '<row r="1"><c r="A1"><f>SUM(</f><v>0</v></c></row>') });
  const unsupported = syntheticWorkbook({ "_xmlsignatures/sig1.xml": "<Signature/>" });
  for (const [bytes, optionsJson] of [
    [Buffer.from("not a ZIP workbook"), "{}"], [malformedFormula, "{}"], [unsupported, "{}"],
    [stale(), '{"limits":{"maxFormulaCells":0}}'], [stale(), '{"unknownOption":true}']
  ]) {
    await writeFile(input, bytes);
    await writeFile(options, optionsJson);
    await writeFile(output, "keep existing destination");
    const failed = runSource([input, "--options", options, "--output", output]);
    assert.equal(failed.status, 1);
    assert.equal(failed.stdout, "");
    const error = JSON.parse(failed.stderr);
    assert.equal(typeof error.code, "string");
    assert.equal(typeof error.message, "string");
    assert.ok(error.message.length > 0);
    if (optionsJson.includes("unknownOption")) {
      assert.equal(error.code, "INVALID_ARGUMENT");
      assert.match(error.message, /unknown field.*unknownOption/);
    }
    assert.equal(await readFile(output, "utf8"), "keep existing destination");
    assert.deepEqual(await readFile(input), bytes);
  }
});

sourceTest("real generated metadata retains exact error totals separately from samples", async t => {
  const directory = await temporary(t), input = join(directory, "errors.xlsx"), options = join(directory, "options.json");
  const rows = Array.from({ length: 201 }, (_, index) => {
    const row = index + 1;
    return `<row r="${row}"><c r="A${row}"><f>1/0</f><v>0</v></c></row>`;
  }).join("");
  const original = syntheticWorkbook({ [sheetName]: worksheet(rows) });
  await writeFile(input, original);
  await writeFile(options, JSON.stringify({ errorLocationLimit: 3, limits: { maxFormulaCells: 201 } }));
  const receipt = success(runSource([input, "--options", options, "--check"]));
  const summary = receipt.metadata.summary;
  assert.equal(summary.status, "errors_found");
  assert.equal(summary.evaluated, 201);
  assert.equal(summary.errors, 201);
  assert.equal(summary.errorSummary["#DIV/0!"].count, 201);
  assert.equal(summary.errorSummary["#DIV/0!"].locations.length, 3);
  assert.equal(summary.errorSummary["#DIV/0!"].locationsTruncated, 198);
  assert.deepEqual(await readFile(input), original);
});


const modeWorkbook = formula => syntheticWorkbook({
  [sheetName]: worksheet(`<row r="1"><c r="A1"><f>${formula}</f><v>99</v></c><c r="B1"><v>7</v></c></row>`)
});
function sourceMetadata(runtime, bytes, options) {
  const result = runtime.recalculateSourceXlsx(new Uint8Array(bytes), JSON.stringify(options ?? {}));
  try { return JSON.parse(result.metadataJson); } finally { result.free(); }
}
sourceTest("maintainer runtime forwards actual modes with sample zero and complete error controls", async () => {
  const { runtime } = await loadRuntime(developmentModule);
  for (const formula of ['CELL("format",B1)', 'IFERROR(CELL("format",B1),7)',
    "PIVOTBY(B1,B1,B1,SUM)", "IF(TRUE,SUM,AVERAGE)", "HSTACK(SUM,AVERAGE)"]) {
    const bytes = modeWorkbook(formula);
    const metadata = sourceMetadata(runtime, bytes, {errorLocationLimit:0});
    assert.equal(metadata.coverage, "partial", formula);
    assert.ok(metadata.operationDiagnostics.functionModes.length, formula);
    assert.deepEqual(metadata.operationDiagnostics.samples, []);
    assert.ok(metadata.operationDiagnostics.functionModes.some(mode =>
      mode.verification === (formula.includes("CELL") ? "unavailable" : "unverified")), formula);
    const displayed = sourceMetadata(runtime,bytes).operationDiagnostics.samples;
    assert.ok(displayed.some(sample => sample.operation.kind ===
      (formula.includes("CELL") ? "unimplemented_function_mode" : "unverified_function_mode")), formula);
    assert.throws(() => sourceMetadata(runtime, bytes, {requireComplete:true}),
      error => error.code === "CALCULATION_DEFERRED");
  }
  for (const formula of ["1/0", "IFERROR(1/0,7)", 'CELL("invalid",B1)',
    'IF(FALSE,CELL("format",B1),7)', 'CELL("row",B1)',
    "IF(FALSE,IF(TRUE,SUM,AVERAGE),7)"]) {
    assert.equal(sourceMetadata(runtime, modeWorkbook(formula), {requireComplete:true}).coverage, "complete", formula);
  }
});

sourceTest("maintainer runtime physical counters distinguish null from measured zero", async () => {
  const { runtime } = await loadRuntime(developmentModule);
  const bytes = modeWorkbook("1+1");
  assert.equal(sourceMetadata(runtime, bytes).operationDiagnostics.physicalFunctionDispatches, null);
  const measured = sourceMetadata(runtime, bytes, {physicalTelemetry:true}).operationDiagnostics;
  assert.equal(measured.physicalFunctionDispatches, 0);
  assert.equal(measured.physicalUnavailableObservations, 0);
  const unavailable = sourceMetadata(runtime, modeWorkbook("MISSING_FN(1)"), {physicalTelemetry:true}).operationDiagnostics;
  assert.ok(unavailable.physicalUnavailableObservations > 0);
});

sourceTest("maintainer fatal evidence budgets never overwrite source or edited destinations and recover", async t => {
  const { runtime } = await loadRuntime(developmentModule);
  const directory = await temporary(t), input = join(directory, "input.xlsx"), output = join(directory, "output.xlsx");
  const bytes = modeWorkbook("MISSING_FN(1)");
  await writeFile(input, bytes);
  await writeFile(output, "destination sentinel");
  const request = JSON.stringify({recalculate:"always",ops:[{kind:"set_cells",sheet_name:"Sheet1",
    overwrite_formulas:true,cells:{A1:{kind:"formula",formula:"MISSING_FN(1)"}}}]});
  for (const field of ["maxOperationRoots", "maxOperationTextBytes", "maxAffectedWork",
    "maxAffectedRegions", "maxAffectedScratchBytes", "maxOutputBytes"]) {
    const options = JSON.stringify({errorLocationLimit:0,limits:{[field]:0}});
    await assert.rejects(recalculateFile(runtime,input,options,output), error =>
      field === "maxOutputBytes"
        ? error.code === "RECALCULATION_FAILED" && /XLSX output byte limit/.test(error.message)
        : error.code === "ENGINE_ERROR");
    await assert.rejects(editFile(runtime,input,request,output,options));
    assert.deepEqual(await readFile(input), bytes);
    assert.equal(await readFile(output,"utf8"), "destination sentinel");
    assert.equal(sourceMetadata(runtime,bytes).coverage,"partial");
  }
  const strict = sourceMetadata(runtime,bytes,{profile:"strict",
    limits:{maxAffectedWork:0,maxAffectedRegions:0,maxAffectedScratchBytes:0}});
  assert.equal(strict.coverage,"partial");
  assert.equal(strict.operationDiagnostics.dependencyEvidence,null);
  assert.throws(() => sourceMetadata(runtime,bytes,{profile:"strict",requireComplete:true,limits:{maxAffectedWork:0}}),
    error => error.code === "CALCULATION_DEFERRED");
  for (const limits of [{maxOperationRoots:null},{maxOperationRoots:true},{maxOperationRoots:0.5},
    {maxOperationRoots:4294967296},{maxAffectedWork:9007199254740992}]) {
    assert.throws(() => sourceMetadata(runtime, bytes, {limits}), error => error.code === "INVALID_ARGUMENT");
  }
});

sourceTest("maintainer independent dynamic scalar and spill survive written byte reopen", async t => {
  const { runtime } = await loadRuntime(developmentModule);
  const directory = await temporary(t), input = join(directory,"in.xlsx"), output = join(directory,"out.xlsx");
  await writeFile(input, syntheticWorkbook({[sheetName]:worksheet(
    '<row r="1"><c r="A1"><f>MISSING_FN(1)</f><v>99</v></c><c r="B1"><f>INDIRECT("A1")</f><v>88</v></c><c r="D1"><v>7</v></c><c r="E1"><f>INDIRECT("D1")</f><v>66</v></c><c r="F1"><f>E1+1</f><v>55</v></c></row><row r="3"><c r="D3"><v>7</v></c><c r="E3"><f>INDIRECT("D3:D4")</f><v>44</v></c></row><row r="4"><c r="D4"><v>8</v></c></row>'
  )}));
  const receipt = await recalculateFile(runtime,input,"{}",output);
  assert.equal(receipt.metadata.coverage,"partial");
  assert.equal(receipt.metadata.operationDiagnostics.dependencyEvidence.unknownFacts,0);
  assert.equal(receipt.metadata.operationDiagnostics.dependencyEvidence.semanticReadsUnattested,true);
  const reopened = await readFile(output), xml = member(reopened,sheetName);
  assert.match(xml, /<f>INDIRECT\("D1"\)<\/f><v>7<\/v>/);
  assert.match(xml, /<f>E1\+1<\/f><v>8<\/v>/);
  const read = runtime.inspectXlsx(new Uint8Array(reopened), JSON.stringify({
    selection:{kind:"cells",addresses:["Sheet1!A1","Sheet1!B1","Sheet1!E1","Sheet1!F1","Sheet1!E3","Sheet1!E4"]}
  }));
  try {
    const items = JSON.parse(read.metadataJson).items;
    assert.equal(items[0].stored_formula_cache?.value ?? null,null);
    assert.equal(items[1].stored_formula_cache?.value ?? null,null);
    assert.deepEqual(items.slice(2).map(item => item.stored_formula_cache?.value?.value ?? item.source_value?.value),[7,8,7,8]);
  } finally { read.free(); }
});
