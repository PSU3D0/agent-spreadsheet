import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, writeFile, rm, access } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { inflateRawSync } from "node:zlib";
import { loadRuntime, recalculateFile, main } from "../scripts/run.mjs";
import { syntheticWorkbook } from "../scripts/synthetic.mjs";

const cli = fileURLToPath(new URL("../scripts/run.mjs", import.meta.url));
// Test-only selection. Production selection is always an explicit CLI argument.
const developmentModule = process.env.FORMUALIZER_TEST_RUNTIME;
const sourceTest = (name, fn) => test(name, { skip: !developmentModule && "Set FORMUALIZER_TEST_RUNTIME to matching generated source loader" }, fn);
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
  return spawnSync(process.execPath, [cli, "--dev-runtime", developmentModule,
    "source-recalculate", ...args], { encoding: "utf8" });
}
function success(result) {
  assert.equal(result.status, 0, result.stderr);
  return JSON.parse(result.stdout);
}

test("default pinned runtime reports source binding unavailable without a session fallback", async t => {
  const directory = await temporary(t), input = join(directory, "input.xlsx");
  await writeFile(input, stale());
  const pinned = await loadRuntime();
  assert.equal(pinned.version, "0.16.0");
  assert.equal(typeof pinned.runtime.recalculateSourceXlsx, "undefined");
  await assert.rejects(main(["source-recalculate", input]), /Registry 0.16.0 lacks this binding/);
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
  assert.equal(version, null);
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
  assert.equal(checked.runtime.kind, "development");
  assert.equal(checked.runtime.registry_version, null);
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
