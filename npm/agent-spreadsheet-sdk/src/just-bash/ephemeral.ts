/**
 * Ephemeral byte binding for stateless adapters on the local runtime.
 *
 * The CLI-shaped adapters bind workbook bytes to a session for exactly one canonical
 * call, export when the operation mutates, and dispose in reverse order.
 */

import { CanonicalOperationError, CapabilityError, SpreadsheetError } from "../errors.js"
import type { LocalSpreadsheet, LocalWorkbook } from "../local.js"
import type { OperationName } from "../generated/operations.js"
import { descriptorFor } from "../registry.js"

const RESOURCE_COUNTS: Readonly<Record<string, number>> = Object.freeze({
  none: 0,
  single_read: 1,
  single_mutable: 1,
  two_resource: 2
})

function adapterError(code: string, message: string, path: string): Error {
  return Object.assign(new Error(message), { aspCode: code, aspPath: path })
}

/** Adapter plan fields the ephemeral flow understands. */
export interface AdapterPlan {
  binding_kind: string
  persistence: string
  support_status: string
}

/** Runtime lifecycle facts the plan gate needs. */
export interface EphemeralRuntimeFacts {
  operations: ReadonlySet<string>
  canExport: boolean
  canDispose: boolean
}

/** True when `plan` can run against a runtime with these lifecycle facts. */
export function supportsEphemeralPlan(plan: AdapterPlan | undefined, facts: EphemeralRuntimeFacts): boolean {
  if (!plan || plan.support_status !== "supported") return false
  if (!Object.hasOwn(RESOURCE_COUNTS, plan.binding_kind)) return false
  if (!["none", "export_required"].includes(plan.persistence)) return false
  if (RESOURCE_COUNTS[plan.binding_kind] > 0 && !facts.canDispose) return false
  return plan.persistence !== "export_required" || facts.canExport
}

/** Operation names the ephemeral adapter can run for `adapter` on this runtime. */
export function availableEphemeralOperations(
  adapter: string,
  facts: EphemeralRuntimeFacts
): Set<string> {
  const available = new Set<string>()
  for (const operation of facts.operations) {
    const plan = descriptorFor(operation)?.adapters[adapter] as AdapterPlan | undefined
    if (supportsEphemeralPlan(plan, facts)) available.add(operation)
  }
  return available
}

/** Reject requests the ephemeral flow cannot honour before any allocation. */
export function validateEphemeralRequest(plan: AdapterPlan, params: unknown): void {
  if (!plan || plan.support_status !== "supported" ||
      !Object.hasOwn(RESOURCE_COUNTS, plan.binding_kind) ||
      !["none", "export_required"].includes(plan.persistence)) {
    throw adapterError("CAPABILITY_UNAVAILABLE", "operation is unavailable in this stateless adapter", "adapter")
  }
  if (!params || typeof params !== "object" || Array.isArray(params)) {
    throw adapterError("INVALID_REQUEST", "canonical request payload must be a JSON object", "$")
  }
  if (plan.persistence === "export_required" && (params as { mode?: unknown }).mode === "stage") {
    throw adapterError(
      "CAPABILITY_UNAVAILABLE",
      "stage requires durable orchestration and is unavailable in a stateless adapter",
      "$.mode"
    )
  }
}

function bindResource(params: Record<string, unknown>, key: string, resourceId: string): void {
  if (params[key] && params[key] !== resourceId) {
    throw adapterError(
      "INVALID_REQUEST",
      `payload ${key} does not match the ephemeral bound resource`,
      `$.${key}`
    )
  }
  params[key] = resourceId
}

/** The result of one ephemeral canonical call. */
export interface EphemeralResult {
  response: unknown
  workbookBytes?: Uint8Array
  /** Artifact bytes, when the operation produced one and the caller wants them. */
  artifactBytes?: Uint8Array
  exitCode: number
}

/** The artifact handle a canonical response carries, when it carries one. */
export function artifactHandleOf(response: unknown): string | undefined {
  const handle = (response as { data?: { artifact?: { handle?: unknown } } })
    ?.data?.artifact?.handle
  return typeof handle === "string" ? handle : undefined
}

/** True when this operation's output schema declares an artifact handle. */
export function producesArtifact(operation: string): boolean {
  const schema = descriptorFor(operation)?.output_schema as {
    $defs?: Record<string, { properties?: Record<string, unknown> }>
    properties?: { data?: { $ref?: string; properties?: Record<string, unknown> } }
  } | undefined
  const data = schema?.properties?.data
  // The generated envelope schema points `data` at a definition rather than
  // inlining it, so the reference is followed here.
  const reference = data?.$ref?.startsWith("#/$defs/")
    ? schema?.$defs?.[data.$ref.slice("#/$defs/".length)]
    : undefined
  return (reference ?? data)?.properties?.["artifact"] !== undefined
}

/** The adapter file flags a request may carry. */
export interface AdapterFlags {
  output: boolean
  inPlace: boolean
  outputIsBind: boolean
}

function flagError(message: string, path: string): Error {
  return adapterError("INVALID_REQUEST", message, path)
}

/**
 * Decide what the `--output` / `--in-place` flags mean for this operation.
 *
 * Three shapes exist: a mutating operation exports a workbook and must name
 * exactly one destination; an artifact operation renders bytes, so `--output`
 * is optional and `--in-place` is meaningless; everything else writes nothing.
 */
export function validateAdapterFlags(
  operation: string,
  plan: AdapterPlan,
  params: Record<string, unknown>,
  flags: AdapterFlags
): { exports: boolean; artifact: boolean } {
  const exports = plan.persistence === "export_required"
  const artifact = !exports && producesArtifact(operation)
  const preview = params["mode"] === "preview"
  if (artifact && flags.inPlace) {
    throw flagError(
      `canonical operation '${operation}' produces an artifact; use --output <VFS_PATH>`,
      "--in-place"
    )
  }
  if (!exports && !artifact && (flags.output || flags.inPlace)) {
    throw flagError(
      "--output and --in-place require a bound workbook-write operation",
      "adapter_flags"
    )
  }
  if (exports && preview && (flags.output || flags.inPlace)) {
    throw flagError("preview does not accept file export flags", "adapter_flags")
  }
  if (exports && !preview && flags.output === flags.inPlace) {
    throw flagError(
      "a mutating operation requires exactly one of --output <VFS_PATH> or --in-place",
      "adapter_flags"
    )
  }
  if (flags.outputIsBind) {
    throw flagError("--output must differ from --bind; use --in-place", "--output")
  }
  return { exports, artifact }
}

/** File generations belong to the byte/VFS boundary, not to a temporary owner. */
async function fileRevision(bytes: Uint8Array): Promise<string> {
  const digest = await globalThis.crypto.subtle.digest("SHA-256", Uint8Array.from(bytes))
  return Array.from(new Uint8Array(digest), byte => byte.toString(16).padStart(2, "0")).join("")
}

/** Bind bytes, dispatch one canonical operation, export when required, dispose. */
export async function executeEphemeralOperation(params: {
  local: LocalSpreadsheet
  operation: string
  params: Record<string, unknown>
  plan: AdapterPlan
  workbooks?: Uint8Array[]
  /** Fetch artifact bytes across the adapter boundary after the call. */
  wantsArtifact?: boolean
}): Promise<EphemeralResult> {
  validateEphemeralRequest(params.plan, params.params)
  const expected = RESOURCE_COUNTS[params.plan.binding_kind]
  const workbooks = params.workbooks ?? []
  if (workbooks.length !== expected) {
    throw adapterError(
      "INVALID_REQUEST",
      `binding kind '${params.plan.binding_kind}' requires ${expected} workbook byte source(s)`,
      "adapter_binding"
    )
  }

  const request: Record<string, unknown> = { ...params.params }
  const opened: LocalWorkbook[] = []
  try {
    for (const bytes of workbooks) opened.push(await params.local.open(bytes))
    if (opened[0]) bindResource(request, "resource_id", opened[0].resourceId)
    if (opened[1]) bindResource(request, "baseline_resource_id", opened[1].resourceId)

    const fileBefore = workbooks[0] ? await fileRevision(workbooks[0]) : undefined
    if (opened[0] && request["expected_revision"] !== undefined) {
      if (request["expected_revision"] !== fileBefore) {
        throw adapterError("REVISION_CONFLICT", "expected_revision does not match the bound file generation", "$.expected_revision")
      }
      const metadata = await opened[0].metadata() as { revision_id: string }
      request["expected_revision"] = metadata.revision_id
    }
    const response = await params.local.canonical.execute(
      params.operation as OperationName,
      request as never
    )
    const status = (response as { data?: { status?: string } })?.data?.status
    const failed = status === "failed" || status === "rolled_back"
    let workbookBytes: Uint8Array | undefined
    if (params.plan.persistence === "export_required" && request["mode"] !== "preview" && !failed) {
      workbookBytes = await opened[0]!.exportBytes()
    }
    // Only project protocol metadata, never recursively rewrite workbook values.
    // This owner is discarded below; its private resident token must not become
    // the next file command's CAS token. Native file commands also use SHA-256.
    if (fileBefore !== undefined) {
      const fileAfter = workbookBytes ? await fileRevision(workbookBytes) : fileBefore
      const envelope = response as any
      envelope.revision_id = fileAfter
      const data = envelope.data
      if (data && typeof data === "object") {
        if ("revision_before" in data) data.revision_before = fileBefore
        if ("revision_after" in data) data.revision_after = fileAfter
        if (data.calculation && "revision_id" in data.calculation) data.calculation.revision_id = fileAfter
      }
    }
    let artifactBytes: Uint8Array | undefined
    const handle = artifactHandleOf(response)
    if (params.wantsArtifact && handle && opened[0]) {
      // The bytes cross here, at the adapter boundary, and the session slot is
      // released on the way out.
      artifactBytes = await opened[0].readArtifact(handle)
    }
    return {
      response,
      workbookBytes,
      artifactBytes,
      exitCode: status === "partial" ? 2 : failed ? 1 : 0
    }
  } finally {
    for (const workbook of opened.reverse()) {
      try { await workbook.dispose() } catch { /* best effort */ }
    }
  }
}

/** Project any SDK error onto the adapter's canonical envelope shape. */
export function adapterEnvelopeFor(error: unknown): unknown {
  if (error instanceof CanonicalOperationError) return error.envelope
  if (error instanceof CapabilityError) {
    return {
      schema_version: "1",
      error: {
        code: "CAPABILITY_UNAVAILABLE",
        message: error.message,
        ...(error.operation ? { operation: error.operation } : {}),
        path: "adapter"
      }
    }
  }
  if (error instanceof SpreadsheetError) {
    return {
      schema_version: "1",
      error: { code: "OPERATION_FAILED", message: error.message }
    }
  }
  return undefined
}
