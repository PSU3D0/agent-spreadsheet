// A fake `agent-spreadsheet-wasm` bindings object for unit tests.

const DEFAULT_OPERATIONS = [
  "describe_workbook",
  "list_sheets",
  "read_cells",
  "screenshot_sheet",
  "write",
  "recalculate",
  "verify_workbook"
]

function envelope(operation, resourceId, data, revisionId = "rev-1") {
  return {
    schema_version: "1",
    operation,
    resource_id: resourceId,
    revision_id: revisionId,
    data
  }
}

function errorEnvelope(code, message, operation, path) {
  return {
    schema_version: "1",
    error: { code, message, operation, path }
  }
}

/**
 * @param {{
 *   operations?: string[],
 *   readArtifact?: boolean,
 *   exportWorkbook?: boolean,
 *   respond?: (call: object) => unknown
 * }} [options]
 */
function createFakeBindings(options = {}) {
  const state = {
    created: [],
    executed: [],
    exported: [],
    disposed: [],
    artifacts: [],
    disposedArtifacts: []
  }
  let sequence = 0
  let revision = 1

  const bindings = {
    operations() {
      return JSON.stringify(
        (options.operations ?? DEFAULT_OPERATIONS).map((name) => ({ name, available: true }))
      )
    },
    sessionMetadata(sessionId) { return JSON.stringify({ resource_id: sessionId, revision_id: `rev-${revision}`, durability: "memory" }) },
    createSession(bytes) {
      const id = `session:s${++sequence}`
      state.created.push({ id, bytes })
      return id
    },
    async executeOperation(sessionId, operation, paramsJson) {
      const params = JSON.parse(paramsJson)
      const call = { sessionId, operation, params }
      state.executed.push(call)
      if (options.respond) {
        const custom = options.respond(call)
        if (custom !== undefined) {
          if (custom instanceof Error) throw custom
          return typeof custom === "string" ? custom : JSON.stringify(custom)
        }
      }
      if (params.expected_revision !== undefined && params.expected_revision !== `rev-${revision}`) {
        throw JSON.stringify(errorEnvelope(
          "REVISION_CONFLICT",
          `expected ${params.expected_revision}`,
          operation,
          "$.expected_revision"
        ))
      }
      if (operation === "write" || operation === "recalculate") revision += 1
      if (operation === "screenshot_sheet") {
        return JSON.stringify(envelope(operation, sessionId, {
          sheet_name: params.sheet_name,
          range: params.range ?? "A1:D20",
          artifact: {
            handle: `artifact:sha256:${"a".repeat(64)}`,
            hash: "a".repeat(64),
            bytes: 4,
            media_type: "image/png"
          },
          duration_ms: 1,
          width: 640,
          height: 480,
          png_level: params.png_level ?? "balanced",
          fidelity: "approximate",
          warnings: [{ code: "font_substituted", message: "Calibri" }],
          calculation: { state: "cached" },
          renderer: "native"
        }, `rev-${revision}`))
      }
      return JSON.stringify(envelope(operation, sessionId, { ok: operation }, `rev-${revision}`))
    },
    exportWorkbook(sessionId) {
      state.exported.push(sessionId)
      return Uint8Array.from([80, 75, 3, 4])
    },
    disposeSession(sessionId) {
      state.disposed.push(sessionId)
      return true
    }
  }

  if (options.readArtifact !== false) {
    bindings.readArtifact = (sessionId, handle) => {
      state.artifacts.push({ sessionId, handle })
      return Uint8Array.from([137, 80, 78, 71])
    }
    bindings.disposeArtifact = (sessionId, handle) => {
      state.disposedArtifacts.push({ sessionId, handle })
      return true
    }
  }
  if (options.exportWorkbook === false) delete bindings.exportWorkbook

  return { bindings, state }
}

module.exports = { createFakeBindings, envelope, errorEnvelope, DEFAULT_OPERATIONS }
