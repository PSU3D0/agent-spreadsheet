import type { CanonicalErrorCode, CanonicalErrorEnvelope } from "./generated/operations.js"

/**
 * HTTP status for each canonical error code, as pinned by
 * `docs/architecture/canonical-http-route.md`.
 */
export const CANONICAL_ERROR_STATUS: Readonly<Record<CanonicalErrorCode, number>> = Object.freeze({
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
})

/** The HTTP status the canonical route uses for `code`, or `undefined` when unknown. */
export function statusForCanonicalCode(code: string): number | undefined {
  return (CANONICAL_ERROR_STATUS as Record<string, number>)[code]
}

/** Base class for every error this SDK throws. */
export class SpreadsheetError extends Error {
  constructor(message: string, options?: { cause?: unknown }) {
    super(message)
    this.name = "SpreadsheetError"
    if (options && "cause" in options) (this as { cause?: unknown }).cause = options.cause
  }
}

/** A canonical error envelope returned by an adapter, decoded into an error. */
export class CanonicalOperationError extends SpreadsheetError {
  /** Canonical error code, for example `REVISION_CONFLICT`. */
  readonly code: string
  /** The operation the dispatcher attributed the failure to. */
  readonly operation: string | undefined
  /** JSON pointer-ish path into the request, when the dispatcher reported one. */
  readonly path: string | undefined
  /** Transport-level context (HTTP status, runtime kind). */
  readonly details: Record<string, unknown>
  /** The unmodified canonical error envelope. */
  readonly envelope: CanonicalErrorEnvelope

  constructor(
    envelope: CanonicalErrorEnvelope,
    details: Record<string, unknown> = {}
  ) {
    super(envelope.error.message)
    this.name = "CanonicalOperationError"
    this.code = envelope.error.code
    this.operation = envelope.error.operation ?? undefined
    this.path = envelope.error.path ?? undefined
    this.details = details
    this.envelope = envelope
  }

  /** The HTTP status the canonical route maps this code to. */
  get canonicalStatus(): number | undefined {
    return statusForCanonicalCode(this.code)
  }
}

/** A call that the live runtime does not support, thrown before any transport. */
export class CapabilityError extends SpreadsheetError {
  /** The canonical operation or SDK capability that is missing. */
  readonly capability: string
  /** The operation name, when the capability is an operation. */
  readonly operation: string | undefined
  /** Operations the runtime does advertise. */
  readonly available: readonly string[]

  constructor(params: {
    capability: string
    operation?: string
    available?: readonly string[]
    message?: string
  }) {
    super(
      params.message ??
        `this runtime does not support '${params.capability}'`
    )
    this.name = "CapabilityError"
    this.capability = params.capability
    this.operation = params.operation
    this.available = Object.freeze([...(params.available ?? [])])
  }
}

/** A non-canonical transport failure: an unreachable host, or a body that is not an envelope. */
export class TransportError extends SpreadsheetError {
  /** HTTP status, or 0 when the request never produced a response. */
  readonly status: number
  /** Raw response body, truncated by the caller when large. */
  readonly body: string | undefined
  /** The operation being attempted, when known. */
  readonly operation: string | undefined

  constructor(
    message: string,
    params: { status?: number; body?: string; operation?: string; cause?: unknown } = {}
  ) {
    super(message, { cause: params.cause })
    this.name = "TransportError"
    this.status = params.status ?? 0
    this.body = params.body
    this.operation = params.operation
  }
}

/** True when `value` is a canonical error envelope. */
export function isCanonicalErrorEnvelope(value: unknown): value is CanonicalErrorEnvelope {
  if (!value || typeof value !== "object") return false
  const candidate = value as { schema_version?: unknown; error?: unknown }
  if (candidate.schema_version !== "1") return false
  const error = candidate.error as { code?: unknown; message?: unknown } | undefined
  return Boolean(error) && typeof error?.code === "string" && typeof error?.message === "string"
}

/** Build a canonical envelope for a failure the SDK itself detected. */
export function canonicalEnvelope(
  code: CanonicalErrorCode,
  message: string,
  operation?: string,
  path?: string
): CanonicalErrorEnvelope {
  const error: CanonicalErrorEnvelope["error"] = { code, message }
  if (operation !== undefined) error.operation = operation
  if (path !== undefined) error.path = path
  return { schema_version: "1", error }
}

/**
 * Decode an adapter rejection into a `CanonicalOperationError`.
 *
 * The WASM binding rejects with a JSON string carrying the canonical envelope; the HTTP
 * route returns the same envelope as a body. Anything else becomes a `TransportError`.
 */
export function decodeRejection(
  rejection: unknown,
  context: { operation: string; runtime: string }
): SpreadsheetError {
  if (rejection instanceof SpreadsheetError) return rejection

  let decoded: unknown = rejection
  if (typeof rejection === "string") {
    try {
      decoded = JSON.parse(rejection)
    } catch {
      return new TransportError(rejection, { operation: context.operation })
    }
  }

  if (isCanonicalErrorEnvelope(decoded)) {
    return new CanonicalOperationError(decoded, {
      runtime: context.runtime,
      operation: context.operation
    })
  }

  if (decoded instanceof Error) {
    return new TransportError(decoded.message, {
      operation: context.operation,
      cause: decoded
    })
  }

  return new TransportError(
    `${context.runtime} runtime rejected '${context.operation}' with a non-canonical value`,
    { operation: context.operation, cause: rejection }
  )
}
