const test = require("node:test")
const assert = require("node:assert/strict")
const { execFileSync } = require("node:child_process")
const fs = require("node:fs")
const path = require("node:path")

const manifest = require("../src/generated/canonical-registry.json")

function aspBinary() {
  const candidates = [
    process.env.ASP_BINARY,
    path.resolve(__dirname, "..", "..", "..", "target", "debug", "asp"),
    path.resolve(__dirname, "..", "..", "..", "target", "release", "asp")
  ].filter(Boolean)
  return candidates.find((candidate) => fs.existsSync(candidate))
}

function run(asp, args) {
  return JSON.parse(execFileSync(asp, args, { encoding: "utf8" }))
}

function cliInputSchema(descriptor) {
  const schema = structuredClone(descriptor.input_schema)
  const injected = ["resource_id"]
  if (descriptor.name === "verify_workbook") injected.push("baseline_resource_id")
  if (Array.isArray(schema.required)) {
    schema.required = schema.required.filter((field) => !injected.includes(field))
  }
  for (const field of injected) {
    if (!schema.properties?.[field]) continue
    const flag = field === "baseline_resource_id" ? "--baseline" : "--bind"
    schema.properties[field].description =
      `Injected by the CLI adapter from ${flag}; omit this field from operation JSON.`
  }
  return schema
}

test("checked-in host-independent registry has all 32 asp operations", (t) => {
  const asp = aspBinary()
  if (!asp) {
    t.skip("set ASP_BINARY (or build target/debug/asp) to run registry drift verification")
    return
  }

  const actual = run(asp, ["registry", "--all"])
  const { generated_by: _generatedBy, ...checkedIn } = manifest
  assert.deepEqual(checkedIn, actual, "run npm run generate:registry after registry changes")
  assert.equal(actual.operations.length, 32)

  for (const descriptor of actual.operations) {
    const schema = run(asp, ["schema", descriptor.name])
    assert.deepEqual(cliInputSchema(descriptor), schema.input_schema, `${descriptor.name} CLI input projection drifted`)
    assert.deepEqual(descriptor.output_schema, schema.output_schema, `${descriptor.name} output schema drifted`)
    assert.deepEqual(actual.error_schema, schema.error_schema, `${descriptor.name} error schema drifted`)
  }
})

const EXPECTED_WASM_OPERATIONS = [
  "describe_workbook",
  "list_sheets",
  "sheet_overview",
  "read_cells",
  "inspect_cells",
  "read_table",
  "read_layout",
  "export_grid",
  "named_ranges",
  "analyze_styles",
  "search_values",
  "search_formulas",
  "formula_trace",
  "formula_map",
  "profile_table",
  "sheet_statistics",
  // 5006: the raster renderer compiles to wasm32, so the byte/session adapters
  // render too. Bytes leave through readArtifact / the just-bash VFS, never
  // through the canonical envelope.
  "screenshot_sheet",
  "write",
  "recalculate",
  "verify_workbook",
  "checkpoint", "staged_change", "session_history"
]

function supported(adapter) {
  return manifest.operations
    .filter((descriptor) => descriptor.adapters[adapter].support_status === "supported")
    .map((descriptor) => descriptor.name)
}

test("checked-in MCP adapter subset remains intentional", () => {
  assert.deepEqual(supported("mcp"), manifest.operations.map(({ name }) => name))
  assert.equal(supported("mcp").length, 32)
})

test("checked-in WASM and just-bash adapter subsets remain intentional", () => {
  assert.deepEqual(supported("wasm"), EXPECTED_WASM_OPERATIONS)
  assert.deepEqual(supported("just_bash"), EXPECTED_WASM_OPERATIONS)

  for (const descriptor of manifest.operations) {
    const plan = descriptor.adapters.just_bash
    assert.equal(plan.binding_kind, descriptor.adapters.wasm.binding_kind, descriptor.name)
    if (EXPECTED_WASM_OPERATIONS.includes(descriptor.name)) {
      assert.equal(
        plan.persistence,
        ["write", "recalculate"].includes(descriptor.name) ? "export_required" :
          ["checkpoint", "staged_change", "session_history"].includes(descriptor.name) ? "resident_required" : "none",
        descriptor.name
      )
    }
  }
})
