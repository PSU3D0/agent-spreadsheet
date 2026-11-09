import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { inflateRawSync } from "node:zlib";
import { syntheticWorkbook } from "../scripts/synthetic.mjs";
import { editFile, loadRuntime, main } from "../scripts/xlsx.mjs";
const cli = fileURLToPath(new URL("../scripts/xlsx.mjs", import.meta.url));
const loader = process.env.FORMUALIZER_TEST_RUNTIME;
const real = (name, fn) => test(name, fn);
const set = value => ({ kind: "set_cells", sheet_name: "Sheet1", cells: { A1: { kind: "value", value } } });
async function temporary(t) {
  const path = await mkdtemp(join(tmpdir(), "formualizer-edit-"));
  t.after(() => rm(path, { recursive: true, force: true }));
  return path;
}
function run(args) {
  const prefix = loader ? ["--dev-runtime", loader] : [];
  return spawnSync(process.execPath, [cli, ...prefix, "edit", ...args], { encoding: "utf8" });
}
function success(result) {
  assert.equal(result.status, 0, result.stderr);
  return JSON.parse(result.stdout);
}
function sheet(bytes) {
  let end = bytes.length - 22;
  while (end >= 0 && bytes.readUInt32LE(end) !== 0x06054b50) end--;
  let offset = bytes.readUInt32LE(end + 16);
  for (let index = 0; index < bytes.readUInt16LE(end + 10); index++) {
    const size = bytes.readUInt32LE(offset + 20), length = bytes.readUInt16LE(offset + 28);
    const name = bytes.subarray(offset + 46, offset + 46 + length).toString();
    if (name === "xl/worksheets/sheet1.xml") {
      const local = bytes.readUInt32LE(offset + 42);
      const start = local + 30 + bytes.readUInt16LE(local + 26) + bytes.readUInt16LE(local + 28);
      const content = bytes.subarray(start, start + size);
      return (bytes.readUInt16LE(offset + 10) === 8 ? inflateRawSync(content) : content).toString();
    }
    offset += 46 + length + bytes.readUInt16LE(offset + 30) + bytes.readUInt16LE(offset + 32);
  }
  assert.fail("sheet missing");
}
function cached(bytes, address) {
  const cell = sheet(bytes).match(new RegExp('<c\\b[^>]*r="' + address + '"[^>]*>([\\s\\S]*?)</c>'));
  assert.ok(cell, address);
  return cell[1].match(/<v>([^<]*)<\/v>/)?.[1];
}
test("pinned registry 0.17.0 exposes editXlsx while rejecting missing binding without resident fallback", async t => {
  const directory = await temporary(t), input = join(directory, "input.xlsx");
  await writeFile(input, syntheticWorkbook());
  const { runtime, version } = await loadRuntime();
  assert.equal(version, "0.17.0-stateless-experiment");
  assert.equal(typeof runtime.editXlsx, "function");
  await assert.rejects(editFile({}, input, "{}", join(directory, "out.xlsx")), /does not export editXlsx/);
});
test("edit result is freed on decoding/write errors; CLI needs explicit output", async t => {
  const directory = await temporary(t), input = join(directory, "input.xlsx");
  await writeFile(input, syntheticWorkbook());
  for (const malformed of [false, true]) {
    let freed = 0;
    const runtime = { createSession() { assert.fail("resident construction"); },
      editXlsx() { return { bytes: new Uint8Array([1]), metadataJson: malformed ? "{" : "{}",
        free() { freed++; } }; } };
    await assert.rejects(editFile(runtime, input, "{}", malformed ? undefined : join(directory, "absent", "out.xlsx")));
    assert.equal(freed, 1);
  }
  await assert.rejects(main(["edit", input, "--operations", "request.json"]), /requires --operations.*--output/);
});
real("real direct batch modes edit A1, recalc when selected and never require session handles", async t => {
  const directory = await temporary(t), input = join(directory, "input.xlsx");
  const original = syntheticWorkbook();
  await writeFile(input, original);
  const { runtime } = await loadRuntime(loader);
  for (const mode of ["never", "auto", "always"]) {
    const output = join(directory, mode + ".xlsx");
    let freed = 0;
    const receipt = await editFile({
      createSession() { assert.fail("edit path created resident session"); },
      editXlsx(...args) {
        const actual = runtime.editXlsx(...args);
        return { get bytes() { return actual.bytes; }, get metadataJson() { return actual.metadataJson; },
          free() { freed++; actual.free(); } };
      }
    }, input, JSON.stringify({ ops: [set(30)], recalculate: mode }), output);
    assert.equal(freed, 1);
    assert.equal(cached(await readFile(output), "A1"), "30");
    assert.equal(receipt.metadata.edit.calculation.performed, mode !== "never");
    assert.equal(receipt.metadata.edit.ops_applied, 1);
    assert.equal(receipt.metadata.edit.details, undefined);
    assert.equal(receipt.metadata.edit.change_count, null);
    assert.equal(receipt.metadata.edit.diff_exact, false);
    assert.equal(receipt.current, undefined);
    if (mode === "never") assert.equal(receipt.metadata.edit.calculation.cache_state, "unknown");
    else {
      assert.equal(cached(await readFile(output), "B1"), "60");
      assert.equal(receipt.metadata.recalculation.summary.evaluated, 1);
    }
    assert.deepEqual(await readFile(input), original);
  }
});
real("two independent CLI invocations reopen output and write a formula with existing overwrite intent", async t => {
  const directory = await temporary(t), input = join(directory, "input.xlsx"),
    first = join(directory, "first.xlsx"), second = join(directory, "second.xlsx"), request = join(directory, "ops.json");
  await writeFile(input, syntheticWorkbook());
  await writeFile(request, JSON.stringify({ ops: [set(30)] }));
  const receipt = success(run([input, "--operations", request, "--output", first]));
  assert.equal(receipt.mode, "edit");
  assert.equal(receipt.runtime.kind, loader ? "development" : "bundled");
  assert.equal(cached(await readFile(first), "B1"), "60");
  await writeFile(request, JSON.stringify({ details: true, ops: [{
    kind: "set_cells", sheet_name: "Sheet1", overwrite_formulas: true,
    cells: { B1: { kind: "formula", formula: "A1*3" } }
  }] }));
  const next = success(run([first, "--operations", request, "--output", second]));
  assert.equal(cached(await readFile(second), "B1"), "90");
  assert.equal(next.metadata.edit.details.length, 1);
  assert.equal(next.metadata.edit.details_omitted, undefined);
  assert.ok(next.metadata.edit.change_count > 0);
  assert.equal(next.metadata.edit.diff_exact, true);
  assert.deepEqual(await readFile(input), syntheticWorkbook());
});
real("Auto skips style/layout while Always recalculates the same presentation batch", async t => {
  const directory = await temporary(t), input = join(directory, "input.xlsx"), request = join(directory, "ops.json");
  await writeFile(input, syntheticWorkbook());
  const ops = [
    { kind: "style", sheet_name: "Sheet1", target: { kind: "range", range: "A1:A1" }, patch: { font: { bold: true } } },
    { kind: "column_size", sheet_name: "Sheet1", target: { kind: "columns", range: "A:A" }, size: { kind: "width", width_chars: 18 } }
  ];
  for (const mode of ["auto", "always"]) {
    await writeFile(request, JSON.stringify({ ops, recalculate: mode }));
    const receipt = success(run([input, "--operations", request, "--output", join(directory, mode + ".xlsx")]));
    assert.equal(receipt.metadata.edit.calculation.performed, mode === "always");
    if (mode === "auto") assert.equal(receipt.metadata.edit.calculation.cache_state, "unknown");
  }
});
real("atomic edit or requested calculation failures retain destination and plain error code/reason", async t => {
  const directory = await temporary(t), input = join(directory, "input.xlsx"), request = join(directory, "ops.json"),
    output = join(directory, "output.xlsx"), options = join(directory, "options.json");
  const original = syntheticWorkbook();
  await writeFile(input, original);
  const missing = { kind: "set_cells", sheet_name: "Missing", cells: { A1: { kind: "value", value: 1 } } };
  const cases = [
    [{ ops: [set(30), missing], recalculate: "never" }, undefined],
    [{ ops: [set(30)], recalculate: "always" }, { limits: { maxFormulaCells: 0 } }],
    [{ ops: [set(30)], recalculate: "sometimes" }, undefined],
    [{ ops: [set(30)], resource_id: "session:caller" }, undefined],
    [{ ops: [set(30)], recalculate: "never" }, { evalConfig: {} }],
    [{ ops: [{ kind: "unknown" }] }, undefined]
  ];
  for (const [body, recalcOptions] of cases) {
    await writeFile(request, JSON.stringify(body));
    await writeFile(output, "keep");
    const args = [input, "--operations", request, "--output", output];
    if (recalcOptions !== undefined) {
      await writeFile(options, JSON.stringify(recalcOptions));
      args.push("--recalc-options", options);
    }
    const failed = run(args);
    assert.equal(failed.status, 1);
    assert.equal(failed.stdout, "");
    const error = JSON.parse(failed.stderr);
    assert.equal(typeof error.code, "string");
    assert.ok(error.code.length > 0);
    assert.equal(typeof error.message, "string");
    assert.ok(error.message.length > 0);
    assert.equal(await readFile(output, "utf8"), "keep");
    assert.deepEqual(await readFile(input), original);
  }
});

real("known style-free rich-writer refusal keeps input and destination with a plain actionable error", async t => {
  const directory = await temporary(t), input = join(directory, "style-free.xlsx"),
    output = join(directory, "output.xlsx"), request = join(directory, "ops.json");
  // No style part/relationship: the known minimal strict-source package limitation.
  const original = syntheticWorkbook({
    "xl/styles.xml": null,
    "[Content_Types].xml": '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>',
    "xl/_rels/workbook.xml.rels": '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>'
  });
  await writeFile(input, original);
  await writeFile(output, "keep");
  await writeFile(request, JSON.stringify({ ops: [set(30)], recalculate: "never" }));
  const failed = run([input, "--operations", request, "--output", output]);
  assert.equal(failed.status, 1);
  assert.equal(failed.stdout, "");
  const error = JSON.parse(failed.stderr);
  assert.equal(typeof error.code, "string");
  assert.ok(error.code.length > 0);
  assert.match(error.message, /invalid emitted style reference/);
  assert.deepEqual(await readFile(input), original);
  assert.equal(await readFile(output, "utf8"), "keep");
});
