import test from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"

import { createWasmRuntime } from "../src/index.js"

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const repositoryRoot = path.resolve(packageRoot, "..", "..")
const fixture = path.join(
  repositoryRoot,
  "crates", "agent-spreadsheet", "tests", "fixtures", "f1", "baseline.xlsx"
)
const built = fs.existsSync(path.join(packageRoot, "pkg", "agent_spreadsheet_wasm_bg.wasm"))

test("the published loader drives a full session round trip", {
  skip: built ? false : "run `npm run build` in npm/agent-spreadsheet-wasm first"
}, async () => {
  const runtime = await createWasmRuntime()

  const workbookBytes = new Uint8Array(fs.readFileSync(fixture))
  const resourceId = runtime.createSession(workbookBytes)
  assert.match(resourceId, /^session:/)

  try {
    const described = JSON.parse(
      await runtime.executeOperation(resourceId, "describe_workbook", JSON.stringify({}))
    )
    assert.equal(described.schema_version, "1")
    assert.equal(described.operation, "describe_workbook")
    assert.equal(described.resource_id, resourceId)
    assert.ok(typeof described.revision_id === "string" && described.revision_id.length > 0)
    assert.equal(described.data.metadata.sheet_count, 1)
    // A resident document has no current file size until export; describing it
    // must not serialize XLSX merely to populate filesystem metadata.
    assert.equal(described.data.metadata.bytes, null)
    assert.equal(JSON.parse(runtime.sessionMetadata(resourceId)).serializations, 0)
    assert.equal(described.data.capabilities.backend.backend, "xlsx_umya")

    const exported = runtime.exportWorkbook(resourceId)
    assert.ok(exported instanceof Uint8Array)
    assert.ok(exported.byteLength > 0)
    // The export is a materialized workbook, so it is a valid zip container.
    assert.deepEqual(Array.from(exported.subarray(0, 2)), [0x50, 0x4b])
  } finally {
    assert.equal(runtime.disposeSession(resourceId), true)
  }

  assert.equal(runtime.disposeSession(resourceId), false)
})

test("repeated instantiation resolves to one runtime", {
  skip: built ? false : "run `npm run build` in npm/agent-spreadsheet-wasm first"
}, async () => {
  const first = await createWasmRuntime()
  const second = await createWasmRuntime()
  assert.equal(first, second)
})
