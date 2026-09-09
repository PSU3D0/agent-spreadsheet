// A fake `fetch` that speaks the canonical /v1 route from
// docs/architecture/canonical-http-route.md.

const DEFAULT_OPERATIONS = [
  "list_workbooks",
  "describe_workbook",
  "list_sheets",
  "read_cells",
  "screenshot_sheet",
  "write",
  "recalculate",
  "verify_workbook",
  "create_fork",
  "list_forks",
  "get_changes",
  "checkpoint",
  "staged_change",
  "export_fork",
  "discard_fork"
]

const STATUS_BY_CODE = {
  INVALID_REQUEST: 400,
  STALE_CURSOR: 400,
  CURSOR_MISMATCH: 400,
  ROW_EXCEEDS_BUDGET: 400,
  UNKNOWN_OPERATION: 404,
  RESOURCE_NOT_FOUND: 404,
  REVISION_CONFLICT: 409,
  OPERATION_FAILED: 500,
  CAPABILITY_UNAVAILABLE: 501,
  OUTCOME_UNKNOWN: 503,
  RECOVERY_REQUIRED: 503
}

function response(status, body, bytes) {
  return {
    ok: status >= 200 && status < 300,
    status,
    async text() {
      return typeof body === "string" ? body : JSON.stringify(body)
    },
    async arrayBuffer() {
      const view = bytes ?? Uint8Array.from([])
      return view.buffer.slice(view.byteOffset, view.byteOffset + view.byteLength)
    }
  }
}

/** A canonical error response with the status the route's table pins for `code`. */
function canonicalFailure(code, message, operation, path) {
  return response(STATUS_BY_CODE[code], {
    schema_version: "1",
    error: { code, message, operation, path }
  })
}

/**
 * @param {{
 *   operations?: string[],
 *   handler?: (call: { operation: string, input: object }) => unknown,
 *   artifact?: Uint8Array | null
 * }} [options]
 */
function createFakeFetch(options = {}) {
  const state = { calls: [], operations: 0, artifacts: [] }
  let forks = 0
  let revision = 1

  const fetch = async (url, init = {}) => {
    const parsed = new URL(url)
    const path = parsed.pathname

    if (path.endsWith("/v1/operations")) {
      state.operations += 1
      return response(200, (options.operations ?? DEFAULT_OPERATIONS).map((name) => ({
        name,
        available: true
      })))
    }

    if (path.includes("/v1/artifacts/")) {
      const handle = decodeURIComponent(path.split("/v1/artifacts/")[1])
      state.artifacts.push(handle)
      if (options.artifact === null) {
        return canonicalFailure("RESOURCE_NOT_FOUND", `unknown artifact ${handle}`)
      }
      return response(200, "", options.artifact ?? Uint8Array.from([137, 80, 78, 71]))
    }

    const operation = decodeURIComponent(path.split("/v1/op/")[1] ?? "")
    const input = init.body ? JSON.parse(init.body) : {}
    state.calls.push({ operation, input, method: init.method })

    if (options.handler) {
      const custom = await options.handler({ operation, input })
      if (custom !== undefined) return custom
    }

    if (operation === "create_fork") {
      const forkId = `fork:f${++forks}`
      return response(200, {
        schema_version: "1",
        operation,
        resource_id: forkId,
        revision_id: `rev-${revision}`,
        data: { source_resource_id: input.resource_id, fork_resource_id: forkId }
      })
    }
    if (["write", "recalculate", "export_fork", "discard_fork"].includes(operation)) {
      if (input.expected_revision !== `rev-${revision}`) {
        return canonicalFailure(
          "REVISION_CONFLICT",
          `expected rev-${revision}`,
          operation,
          "$.expected_revision"
        )
      }
      revision += 1
    }
    if (operation === "screenshot_sheet") {
      return response(200, {
        schema_version: "1",
        operation,
        resource_id: input.resource_id,
        revision_id: `rev-${revision}`,
        data: {
          sheet_name: input.sheet_name,
          range: input.range ?? "A1:D20",
          artifact: {
            handle: `artifact:sha256:${"b".repeat(64)}`,
            hash: "b".repeat(64),
            bytes: 4,
            media_type: "image/png"
          },
          duration_ms: 2
        }
      })
    }
    if (operation === "list_workbooks") {
      return response(200, {
        schema_version: "1",
        operation,
        data: { next_offset: null, workbooks: [{ resource_id: "wb:wb-1" }] }
      })
    }
    return response(200, {
      schema_version: "1",
      operation,
      resource_id: input.resource_id,
      revision_id: `rev-${revision}`,
      data: { ok: operation }
    })
  }

  return { fetch, state, response, canonicalFailure }
}

module.exports = { createFakeFetch, canonicalFailure, response, STATUS_BY_CODE, DEFAULT_OPERATIONS }
