// Worker-mode RPC shim, exercised over an in-process MessageChannel with fake
// bindings. The shim's contract is that the main thread sees exactly the
// WasmBindings surface, and that canonical error envelopes survive the hop.

const test = require("node:test")
const assert = require("node:assert/strict")
const { MessageChannel } = require("node:worker_threads")

const {
  CanonicalOperationError,
  connectBindings,
  createLocalSpreadsheet,
  serveBindings,
  workerSupported,
  WORKER_METHODS
} = require("agent-spreadsheet-sdk")
const { createFakeBindings, errorEnvelope } = require("../test-support/fake-bindings.js")

function channel(bindings) {
  const { port1, port2 } = new MessageChannel()
  serveBindings(port2, bindings)
  port1.unref()
  port2.unref()
  return { port: port1, close: () => { port1.close(); port2.close() } }
}

test("the shim forwards every binding method and returns values verbatim", async (t) => {
  const { bindings, state } = createFakeBindings()
  const { port, close } = channel(bindings)
  t.after(close)
  const remote = connectBindings(port)

  assert.deepEqual([...WORKER_METHODS].sort(), [
    "createSession", "disposeArtifact", "disposeSession", "executeOperation",
    "exportWorkbook", "operations", "readArtifact", "sessionMetadata"
  ])

  const sessionId = await remote.createSession(Uint8Array.from([1, 2, 3]))
  assert.equal(sessionId, "session:s1")
  assert.deepEqual(Array.from(state.created[0].bytes), [1, 2, 3])

  const raw = await remote.executeOperation(sessionId, "list_sheets", JSON.stringify({}))
  assert.equal(JSON.parse(raw).operation, "list_sheets")

  const exported = await remote.exportWorkbook(sessionId)
  assert.deepEqual(Array.from(exported), [80, 75, 3, 4])

  const png = await remote.readArtifact(sessionId, "artifact:sha256:abc")
  assert.deepEqual(Array.from(png), [137, 80, 78, 71])
  assert.equal(await remote.disposeArtifact(sessionId, "artifact:sha256:abc"), true)
  assert.equal(await remote.disposeSession(sessionId), true)
  assert.deepEqual(state.disposed, [sessionId])
})

test("canonical error envelopes survive the worker hop", async (t) => {
  const envelope = errorEnvelope("REVISION_CONFLICT", "stale", "write", "$.expected_revision")
  const { bindings } = createFakeBindings({
    respond: (call) => (call.operation === "write" ? new Error(JSON.stringify(envelope)) : undefined)
  })
  const { port, close } = channel(bindings)
  t.after(close)
  const remote = connectBindings(port)

  await assert.rejects(
    () => remote.executeOperation("session:s1", "write", JSON.stringify({ mode: "apply" })),
    (rejection) => {
      // The far side flattens Error instances to a message; the SDK decodes the
      // canonical envelope out of it exactly as it does on the main thread.
      assert.equal(JSON.parse(rejection.message).error.code, "REVISION_CONFLICT")
      return true
    }
  )
})

test("a missing method on the far side rejects rather than hanging", async (t) => {
  const { bindings } = createFakeBindings({ readArtifact: false })
  const { port, close } = channel(bindings)
  t.after(close)
  const remote = connectBindings(port)

  await assert.rejects(() => remote.readArtifact("session:s1", "artifact:sha256:abc"), (error) => {
    assert.match(error.message, /readArtifact/)
    return true
  })
})

test("the object model drives a worker-backed runtime end to end", async (t) => {
  const { bindings, state } = createFakeBindings()
  const { port, close } = channel(bindings)
  t.after(close)

  const local = createLocalSpreadsheet({ runtime: bindings, worker: { port } })
  t.after(() => local.close())
  const workbook = await local.open(Uint8Array.from([1]))
  assert.equal(local.worker, true)

  const rendered = await workbook.renderSheet({ sheet_name: "Sheet1", png_level: "fast" })
  assert.deepEqual(Array.from(rendered.png), [137, 80, 78, 71])
  assert.equal(rendered.png_level, "fast")
  // The round trip still releases the slot: worker mode changes the transport,
  // never the lifecycle.
  assert.equal(state.disposedArtifacts.length, 1)

  const conflict = createLocalSpreadsheet({ runtime: bindings, worker: { port } })
  t.after(() => conflict.close())
  const second = await conflict.open(Uint8Array.from([1]))
  await assert.rejects(
    () => second.write({
      mode: "apply",
      expected_revision: "rev-nope",
      ops: [{ kind: "set_cells", sheet_name: "Sheet1", cells: { A1: { kind: "value", value: 1 } } }]
    }),
    CanonicalOperationError
  )
})

test("worker mode refuses a runtime it cannot move", async () => {
  const { bindings } = createFakeBindings()
  const local = createLocalSpreadsheet({ runtime: bindings, worker: true })
  await assert.rejects(() => local.capabilities(), (error) => {
    assert.ok(error instanceof TypeError)
    assert.match(error.message, /cannot cross a worker boundary/)
    return true
  })
  assert.equal(workerSupported(), true)
})
