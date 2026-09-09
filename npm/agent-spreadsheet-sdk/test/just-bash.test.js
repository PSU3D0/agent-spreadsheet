const assert = require("node:assert/strict")
const { spawnSync } = require("node:child_process")
const fs = require("node:fs")
const path = require("node:path")
const test = require("node:test")
const fileRevision = bytes => require("node:crypto").createHash("sha256").update(bytes).digest("hex")

const { Bash } = require("just-bash")
const { createAspCommand } = require("agent-spreadsheet-sdk/just-bash")

function canonical(operation, resourceId, data, revisionId = "rev-1") {
  return {
    schema_version: "1",
    operation,
    resource_id: resourceId,
    revision_id: revisionId,
    data
  }
}

function mockBindings() {
  const state = {
    sessions: new Map(),
    created: 0,
    disposed: [],
    executed: [],
    exported: [],
    artifactsRead: [],
    artifactsDisposed: []
  }
  return {
    state,
    bindings: {
      operations() {
        return ["list_sheets", "screenshot_sheet", "write", "recalculate", "verify_workbook"]
      },
      createSession(bytes) {
        const id = `session:s${++state.created}`
        state.sessions.set(id, Uint8Array.from(bytes))
        return id
      },
      sessionMetadata(resourceId) { return JSON.stringify({ resource_id: resourceId, revision_id: "rev-1", durability: "memory" }) },
      readArtifact(resourceId, handle) {
        state.artifactsRead.push({ resourceId, handle })
        return Uint8Array.from([137, 80, 78, 71])
      },
      disposeArtifact(resourceId, handle) {
        state.artifactsDisposed.push({ resourceId, handle })
        return true
      },
      async executeOperation(resourceId, operation, paramsJson) {
        const params = JSON.parse(paramsJson)
        state.executed.push({ resourceId, operation, params })
        if (operation === "list_sheets") {
          if (params.trigger_error) throw JSON.stringify({
            schema_version: "1",
            error: { code: "INVALID_REQUEST", message: "golden error", operation, path: "$.trigger_error" }
          })
          return JSON.stringify(canonical(operation, resourceId, { sheets: [{ name: "Sheet1" }] }))
        }
        if (operation === "screenshot_sheet") {
          return JSON.stringify(canonical(operation, resourceId, {
            sheet_name: params.sheet_name,
            range: params.range ?? "A1:M40",
            artifact: {
              handle: `artifact:sha256:${"b".repeat(64)}`,
              hash: `sha256:${"b".repeat(64)}`,
              bytes: 4,
              media_type: "image/png"
            },
            duration_ms: 1,
            renderer: "native-raster/1",
            fidelity: "full",
            warnings: [],
            width: 320,
            height: 240,
            png_level: params.png_level ?? "balanced",
            calculation: { state: "clean", revision_id: "rev-1" }
          }))
        }
        if (operation === "verify_workbook") {
          return JSON.stringify(canonical(operation, resourceId, {
            proof_status: "proved",
            baseline_resource_id: params.baseline_resource_id,
            current_resource_id: resourceId
          }))
        }
        if (operation === "write") {
          if (params.mode === "preview") {
            return JSON.stringify(canonical(operation, resourceId, { status: "previewed" }))
          }
          if (params.label === "fail") {
            return JSON.stringify(canonical(operation, resourceId, { status: "failed" }))
          }
          state.sessions.set(resourceId, Uint8Array.from([...state.sessions.get(resourceId), 9]))
          const status = params.atomic === false ? "partial" : "applied"
          return JSON.stringify(canonical(operation, resourceId, { status }, "rev-2"))
        }
        state.sessions.set(resourceId, Uint8Array.from([...state.sessions.get(resourceId), 8]))
        return JSON.stringify(canonical(operation, resourceId, { state: "clean" }, "rev-2"))
      },
      exportWorkbook(resourceId) {
        state.exported.push(resourceId)
        return state.sessions.get(resourceId)
      },
      disposeSession(resourceId) {
        state.disposed.push(resourceId)
        state.sessions.delete(resourceId)
        return true
      }
    }
  }
}

function parseJsonOutput(result) {
  assert.equal(result.stderr, "")
  return JSON.parse(result.stdout)
}

test("actual just-bash executes canonical reads from JSON stdin using only its VFS", async () => {
  const { bindings, state } = mockBindings()
  const bash = new Bash({
    files: { "/book.xlsx": Uint8Array.from([1, 2, 3]) },
    customCommands: [createAspCommand({ bindings })]
  })

  const result = await bash.exec("asp op list_sheets --bind /book.xlsx", { stdin: "{}" })
  const response = parseJsonOutput(result)
  assert.equal(response.resource_id, "session:s1")
  assert.equal(state.executed[0].params.resource_id, "session:s1")
  assert.deepEqual(state.disposed, ["session:s1"])
  assert.deepEqual(Array.from(await bash.fs.readFileBuffer("/book.xlsx")), [1, 2, 3])
})

test("registry discovery stays canonical while schema and examples project adapter bindings", async () => {
  const { bindings } = mockBindings()
  const bash = new Bash({ customCommands: [createAspCommand({ bindings })] })

  const operations = parseJsonOutput(await bash.exec("asp operations"))
  assert.deepEqual(operations.map(({ name }) => name), [
    "list_sheets", "screenshot_sheet", "write", "recalculate", "verify_workbook"
  ])
  assert.ok(operations.every(({ input_schema }) => input_schema === undefined))
  const schema = parseJsonOutput(await bash.exec("asp schema read_cells"))
  assert.equal(schema.name, "read_cells")
  assert.equal(schema.input_schema.title, "ReadCellsRequest")
  assert.ok(!schema.input_schema.required.includes("resource_id"))
  assert.match(schema.input_schema.properties.resource_id.description, /--bind/)
  assert.match(schema.adapter_binding.bind, /--bind/)
  const verifySchema = parseJsonOutput(await bash.exec("asp schema verify_workbook"))
  assert.ok(!verifySchema.input_schema.required.includes("resource_id"))
  assert.ok(!verifySchema.input_schema.required.includes("baseline_resource_id"))
  assert.match(verifySchema.adapter_binding.baseline, /--baseline/)
  const example = parseJsonOutput(await bash.exec("asp example list_sheets"))
  assert.equal(example.resource_id, undefined)

  const restricted = mockBindings().bindings
  delete restricted.exportWorkbook
  const readOnly = new Bash({ customCommands: [createAspCommand({ bindings: restricted })] })
  assert.deepEqual(
    parseJsonOutput(await readOnly.exec("asp operations")).map(({ name }) => name),
    ["list_sheets", "screenshot_sheet", "verify_workbook"]
  )
})

test("preview is pure while apply and recalculate export atomically", async () => {
  const { bindings, state } = mockBindings()
  const bash = new Bash({
    files: { "/book.xlsx": Uint8Array.from([1, 2, 3]) },
    customCommands: [createAspCommand({ bindings })]
  })

  const preview = await bash.exec("asp op write --bind /book.xlsx", {
    stdin: JSON.stringify({ expected_revision: fileRevision(await bash.fs.readFileBuffer("/book.xlsx")), mode: "preview", ops: [] })
  })
  assert.equal(parseJsonOutput(preview).data.status, "previewed")
  assert.equal(state.exported.length, 0)
  assert.deepEqual(Array.from(await bash.fs.readFileBuffer("/book.xlsx")), [1, 2, 3])

  const applied = await bash.exec("asp op write --bind /book.xlsx --output /applied.xlsx", {
    stdin: JSON.stringify({ expected_revision: fileRevision(await bash.fs.readFileBuffer("/book.xlsx")), mode: "apply", ops: [] })
  })
  assert.equal(parseJsonOutput(applied).data.status, "applied")
  assert.deepEqual(Array.from(await bash.fs.readFileBuffer("/applied.xlsx")), [1, 2, 3, 9])
  assert.equal((await bash.fs.getAllPaths()).some((path) => path.includes(".asp-tmp-")), false)

  const recalculated = await bash.exec("asp op recalculate --bind /book.xlsx --in-place", {
    stdin: JSON.stringify({ expected_revision: fileRevision(await bash.fs.readFileBuffer("/book.xlsx")) })
  })
  assert.equal(parseJsonOutput(recalculated).data.state, "clean")
  assert.deepEqual(Array.from(await bash.fs.readFileBuffer("/book.xlsx")), [1, 2, 3, 8])
})

test("canonical errors and write completion statuses map to stderr and exit codes", async () => {
  const { bindings, state } = mockBindings()
  const bash = new Bash({
    files: { "/book.xlsx": Uint8Array.from([1]) },
    customCommands: [createAspCommand({ bindings })]
  })

  const error = await bash.exec(
    "asp op list_sheets --bind /book.xlsx --json '{\"trigger_error\":true}'"
  )
  assert.equal(error.exitCode, 1)
  assert.deepEqual(JSON.parse(error.stderr).error, {
    code: "INVALID_REQUEST",
    message: "golden error",
    operation: "list_sheets",
    path: "$.trigger_error"
  })

  const partial = await bash.exec("asp op write --bind /book.xlsx --output /partial.xlsx", {
    stdin: JSON.stringify({ expected_revision: fileRevision(await bash.fs.readFileBuffer("/book.xlsx")), mode: "apply", atomic: false, ops: [] })
  })
  assert.equal(partial.exitCode, 2)
  assert.equal(JSON.parse(partial.stdout).data.status, "partial")
  assert.deepEqual(Array.from(await bash.fs.readFileBuffer("/partial.xlsx")), [1, 9])

  const exportsBefore = state.exported.length
  const failed = await bash.exec("asp op write --bind /book.xlsx --output /failed.xlsx", {
    stdin: JSON.stringify({ expected_revision: fileRevision(await bash.fs.readFileBuffer("/book.xlsx")), mode: "apply", label: "fail", ops: [] })
  })
  assert.equal(failed.exitCode, 1)
  assert.equal(JSON.parse(failed.stdout).data.status, "failed")
  assert.equal(state.exported.length, exportsBefore)
  assert.equal(await bash.fs.exists("/failed.xlsx"), false)
})

test("two-resource verification preserves both typed session ids and disposes both", async () => {
  const { bindings, state } = mockBindings()
  const bash = new Bash({
    files: {
      "/current.xlsx": Uint8Array.from([1]),
      "/baseline.xlsx": Uint8Array.from([2])
    },
    customCommands: [createAspCommand({ bindings })]
  })

  const result = await bash.exec(
    "asp op verify_workbook --bind /current.xlsx --baseline /baseline.xlsx --json '{}'"
  )
  const response = parseJsonOutput(result)
  assert.equal(response.resource_id, "session:s1")
  assert.equal(response.data.baseline_resource_id, "session:s2")
  assert.deepEqual(state.disposed, ["session:s2", "session:s1"])
})

test("structured errors and limits happen before WASM session allocation", async () => {
  const params = mockBindings()
  const tooLargeParams = new Bash({
    files: { "/book.xlsx": Uint8Array.from([1]) },
    customCommands: [createAspCommand({ bindings: params.bindings, maxParamsBytes: 1 })]
  })
  const paramsResult = await tooLargeParams.exec("asp op list_sheets --bind /book.xlsx --json '{}'")
  assert.equal(paramsResult.exitCode, 1)
  assert.equal(JSON.parse(paramsResult.stderr).error.code, "INVALID_REQUEST")
  assert.equal(params.state.created, 0)

  const workbook = mockBindings()
  const tooLargeWorkbook = new Bash({
    files: { "/book.xlsx": Uint8Array.from([1, 2]) },
    customCommands: [createAspCommand({ bindings: workbook.bindings, maxWorkbookBytes: 1 })]
  })
  const workbookResult = await tooLargeWorkbook.exec("asp op list_sheets --bind /book.xlsx --json '{}'")
  assert.equal(JSON.parse(workbookResult.stderr).error.path, "--bind")
  assert.equal(workbook.state.created, 0)

  const missing = await tooLargeWorkbook.exec("asp op list_sheets --json '{}'")
  assert.equal(JSON.parse(missing.stderr).error.path, "--bind")
  const unknown = await tooLargeWorkbook.exec("asp op not_real --json '{'")
  assert.equal(JSON.parse(unknown.stderr).error.code, "UNKNOWN_OPERATION")
  const unavailable = await tooLargeWorkbook.exec("asp op read_cells --bind /book.xlsx --json '{}'")
  assert.equal(JSON.parse(unavailable.stderr).error.code, "CAPABILITY_UNAVAILABLE")
  assert.equal(workbook.state.created, 0)
})

test("same-target exports are locked, no-clobber, and leave no temporary files", async () => {
  const { bindings } = mockBindings()
  const bash = new Bash({
    files: { "/book.xlsx": Uint8Array.from([1]) },
    customCommands: [createAspCommand({ bindings })]
  })
  const command = "asp op write --bind /book.xlsx --output /winner.xlsx"
  const input = { stdin: JSON.stringify({ expected_revision: fileRevision(await bash.fs.readFileBuffer("/book.xlsx")), mode: "apply", ops: [] }) }
  const results = await Promise.all([bash.exec(command, input), bash.exec(command, input)])
  assert.deepEqual(results.map(({ exitCode }) => exitCode).sort(), [0, 1])
  const loser = results.find(({ exitCode }) => exitCode === 1)
  assert.equal(JSON.parse(loser.stderr).error.path, "--output")
  assert.deepEqual(Array.from(await bash.fs.readFileBuffer("/winner.xlsx")), [1, 9])
  assert.equal((await bash.fs.getAllPaths()).some((entry) => entry.includes(".asp-tmp-")), false)

  const inPlace = "asp op write --bind /book.xlsx --in-place"
  const move = bash.fs.mv.bind(bash.fs)
  let activeMoves = 0
  let maxActiveMoves = 0
  bash.fs.mv = async (...args) => {
    activeMoves += 1
    maxActiveMoves = Math.max(maxActiveMoves, activeMoves)
    await new Promise((resolve) => setTimeout(resolve, 5))
    try { return await move(...args) } finally { activeMoves -= 1 }
  }
  const applied = await Promise.all([bash.exec(inPlace, input), bash.exec(inPlace, input)])
  bash.fs.mv = move
  assert.deepEqual(applied.map(({ exitCode }) => exitCode).sort(), [0, 1])
  assert.equal(maxActiveMoves, 1)
  assert.equal((await bash.fs.getAllPaths()).some((entry) => entry.includes(".asp-tmp-")), false)

  bash.fs.mv = async () => { throw new Error("injected move failure") }
  const failed = await bash.exec(
    "asp op write --bind /book.xlsx --output /broken.xlsx",
    { stdin: JSON.stringify({ expected_revision: fileRevision(await bash.fs.readFileBuffer('/book.xlsx')), mode: 'apply', ops: [] }) }
  )
  bash.fs.mv = move
  assert.equal(JSON.parse(failed.stderr).error.path, "adapter_export")
  assert.equal(await bash.fs.exists("/broken.xlsx"), false)
  assert.equal((await bash.fs.getAllPaths()).some((entry) => entry.includes(".asp-tmp-")), false)
})

test("just-bash command remains a thin protocol and VFS transport", () => {
  const source = fs.readFileSync(path.join(__dirname, "..", "src", "just-bash", "index.ts"), "utf8")
  const semanticLines = source.split("\n").filter((line) => {
    const trimmed = line.trim()
    return trimmed && !trimmed.startsWith("//")
  })
  assert.ok(semanticLines.length <= 150, `${semanticLines.length} semantic lines exceeds 150`)
})

test("js-exec child_process bridge invokes asp without a second tools projection", () => {
  const runner = path.join(__dirname, "..", "test-support", "just-bash-js-exec.mjs")
  const result = spawnSync(process.execPath, [runner], { encoding: "utf8", timeout: 10_000 })
  assert.equal(result.status, 0, result.stderr || result.error?.message)
  assert.equal(JSON.parse(result.stdout).operation, "list_sheets")
})

test("screenshot_sheet writes artifact bytes to the VFS and prints the envelope", async () => {
  const { bindings, state } = mockBindings()
  const bash = new Bash({
    files: { "/book.xlsx": Uint8Array.from([1, 2, 3]) },
    customCommands: [createAspCommand({ bindings })]
  })

  const rendered = await bash.exec(
    "asp op screenshot_sheet --bind /book.xlsx --output /shot.png",
    { stdin: JSON.stringify({ sheet_name: "Sheet1", range: "A1:C6", png_level: "fast" }) }
  )
  assert.equal(rendered.exitCode, 0)
  const envelope = parseJsonOutput(rendered)
  assert.equal(envelope.operation, "screenshot_sheet")
  assert.equal(envelope.data.renderer, "native-raster/1")
  assert.equal(envelope.data.png_level, "fast")
  assert.equal(envelope.data.artifact.media_type, "image/png")
  // The envelope carries the handle; the bytes landed in the VFS.
  assert.equal(rendered.stdout.includes("\u0089PNG"), false)
  assert.deepEqual(
    Array.from(await bash.fs.readFileBuffer("/shot.png")),
    [137, 80, 78, 71]
  )
  assert.equal(state.artifactsRead.length, 1)
  assert.equal(state.artifactsRead[0].handle, envelope.data.artifact.handle)
  // The slot is released as part of the read, and the session is disposed after.
  assert.equal(state.artifactsDisposed.length, 1)
  assert.equal(state.disposed.length, 1)
})

test("screenshot_sheet without --output still renders, and rejects --in-place", async () => {
  const { bindings, state } = mockBindings()
  const bash = new Bash({
    files: { "/book.xlsx": Uint8Array.from([1, 2, 3]) },
    customCommands: [createAspCommand({ bindings })]
  })

  const printed = await bash.exec("asp op screenshot_sheet --bind /book.xlsx", {
    stdin: JSON.stringify({ sheet_name: "Sheet1" })
  })
  assert.equal(printed.exitCode, 0)
  assert.equal(parseJsonOutput(printed).data.sheet_name, "Sheet1")
  // Nothing asked for the bytes, so they never crossed the boundary.
  assert.equal(state.artifactsRead.length, 0)

  const inPlace = await bash.exec("asp op screenshot_sheet --bind /book.xlsx --in-place", {
    stdin: JSON.stringify({ sheet_name: "Sheet1" })
  })
  assert.equal(inPlace.exitCode, 1)
  const error = JSON.parse(inPlace.stderr).error
  assert.equal(error.code, "INVALID_REQUEST")
  assert.equal(error.path, "--in-place")
  assert.match(error.message, /produces an artifact/)

  const clobber = new Bash({
    files: { "/book.xlsx": Uint8Array.from([1, 2, 3]), "/shot.png": Uint8Array.from([0]) },
    customCommands: [createAspCommand({ bindings })]
  })
  const existing = await clobber.exec(
    "asp op screenshot_sheet --bind /book.xlsx --output /shot.png",
    { stdin: JSON.stringify({ sheet_name: "Sheet1" }) }
  )
  assert.equal(existing.exitCode, 1)
  assert.equal(JSON.parse(existing.stderr).error.path, "--output")
})
