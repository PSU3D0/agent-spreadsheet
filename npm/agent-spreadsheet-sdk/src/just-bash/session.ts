import type { LocalSpreadsheet, LocalWorkbook } from "../local.js"
import { canonicalError, invalid, jsonResult, parseOperationArgs, parseOperationParams, utf8Bytes } from "./parser.js"
import { readWorkbook } from "./vfs.js"

/** Handle routing only. Rust owns revisions, history, receipts, and calculation. */
export function createSessionCommand(
  local: LocalSpreadsheet,
  maxWorkbookBytes: number,
  maxParamsBytes: number,
  write: (ctx: any, target: string, bytes: Uint8Array, replace: boolean) => Promise<void>
): { execute: (args: string[], ctx: any) => Promise<ReturnType<typeof jsonResult>>; dispose: () => Promise<void> } {
  // A command can be registered with multiple Bash instances. Never allow a
  // session ID from one VFS to become a handle in another one's namespace.
  const scopes = new WeakMap<object, Map<string, LocalWorkbook>>()
  const live = new Set<LocalWorkbook>()
  async function cleanup(books: Iterable<LocalWorkbook>, handles?: Map<string, LocalWorkbook>) {
    const results = await Promise.allSettled([...books].map(async workbook => {
      await workbook.dispose()
      live.delete(workbook)
      handles?.delete(workbook.resourceId)
    }))
    const failures = results.filter((result): result is PromiseRejectedResult => result.status === "rejected")
    // Retain failed handles for retry: losing the only handle would leak the owner.
    if (failures.length) throw new AggregateError(failures.map(result => result.reason), `${failures.length} session cleanup(s) failed; retry cleanup`)
  }
  const execute = async (args: string[], ctx: any) => {
    const scope = ctx.fsIdentity ?? ctx.fs
    let handles = scopes.get(scope)
    if (!handles) { handles = new Map(); scopes.set(scope, handles) }
    const [action, id, ...rest] = args
    if (action === "operations" && args.length === 1) return jsonResult(await local.capabilities())
    if (action === "open") {
      if (args.length !== 2) invalid("usage: asp session open VFS_PATH")
      const workbook = await local.open(await readWorkbook(ctx, id, maxWorkbookBytes, "path"))
      try {
        const metadata = await workbook.metadata()
        handles.set(workbook.resourceId, workbook)
        live.add(workbook)
        return jsonResult(metadata)
      } catch (error) { await workbook.dispose(); throw error }
    }
    if (action === "close" && id === "--all" && rest.length === 0) {
      await cleanup(handles.values(), handles)
      return jsonResult({ closed: true })
    }
    if (!["info", "op", "export", "artifact", "close"].includes(action) || !id) {
      invalid("usage: asp session open|info|op|export|artifact|close|operations ...")
    }
    const workbook = handles.get(id)
    if (!workbook) throw canonicalError("RESOURCE_NOT_FOUND", "session is not open in this VFS scope", undefined, "session_id")
    if (action === "info" || action === "close") {
      if (rest.length) invalid(`asp session ${action} accepts only a session ID`)
      if (action === "info") return jsonResult(await workbook.metadata())
      await workbook.dispose()
      handles.delete(id)
      live.delete(workbook)
      return jsonResult({ resource_id: id, closed: true })
    }
    if (action === "export" || action === "artifact") {
      const flags = action === "artifact" ? rest.slice(1) : rest
      if (flags.length !== 2 || flags[0] !== "--output" || !flags[1] || (action === "artifact" && !rest[0])) {
        invalid(`usage: asp session ${action} SESSION_ID${action === "artifact" ? " HANDLE" : ""} --output NEW_VFS_PATH`)
      }
      // Export is a snapshot, not a persisted journal or a session close.
      // Save-as only: no claim of compare-and-swap against external VFS writers.
      const bytes = action === "artifact"
        ? await workbook.readArtifact(rest[0], { release: false }) : await workbook.exportBytes()
      await write(ctx, flags[1], bytes, false)
      return jsonResult({ resource_id: id, output: flags[1], bytes: bytes.byteLength })
    }
    const opts = parseOperationArgs(["op", ...rest], true)
    if (opts.bind || opts.output || opts.inPlace) invalid("resident operations use session IDs; export separately", opts.operation)
    if (opts["request-id"] !== undefined && (utf8Bytes(opts["request-id"]) < 1 || utf8Bytes(opts["request-id"]) > 256)) invalid("request ID must be 1–256 UTF-8 bytes", opts.operation)
    const params = parseOperationParams(opts.json, ctx.stdin, maxParamsBytes, opts.operation)
    if (!params || typeof params !== "object" || Array.isArray(params)) invalid("params must be an object", opts.operation)
    if (params.resource_id !== undefined && params.resource_id !== id) invalid("resource_id must match the session ID", opts.operation)
    if (opts.baseline) {
      if (params.baseline_resource_id !== undefined && params.baseline_resource_id !== opts.baseline) invalid("baseline IDs disagree", opts.operation)
      params.baseline_resource_id = opts.baseline
    }
    if (params.baseline_resource_id !== undefined && !handles.has(params.baseline_resource_id)) {
      throw canonicalError("RESOURCE_NOT_FOUND", "baseline session is not open in this VFS scope", opts.operation)
    }
    const response = await local.canonical.execute(opts.operation, { ...params, resource_id: id }, { requestId: opts["request-id"] })
    const status = (response as any).data?.status
    return jsonResult(response, status === "partial" ? 2 : ["failed", "rolled_back"].includes(status) ? 1 : 0)
  }
  return { execute, dispose: () => cleanup(live) }
}
