import { CapabilityError, TransportError } from "./errors.js"
import type { InputOf, OperationName, OutputOf } from "./generated/operations.js"
import { isOperationName } from "./registry.js"

/**
 * A canonical dispatcher behind one transport.
 *
 * Both runtimes speak the same protocol: an operation name plus a closed canonical
 * input object in, a canonical response envelope out. No runtime reshapes semantics.
 */
export interface DispatchOptions {
  /** Stable identity for reconciling an exact retry; not a spreadsheet input field. */
  requestId?: string
}

export interface CanonicalRuntime {
  /** Runtime identity, `"local"` or `"server"`. */
  readonly kind: string
  /** Operation names this live runtime advertises. */
  operations(): Promise<ReadonlySet<string>>
  /** Dispatch one canonical operation and return its response envelope. */
  dispatch<K extends OperationName>(
    operation: K,
    input: Record<string, unknown>,
    options?: DispatchOptions
  ): Promise<OutputOf<K>>
  /** Fetch the bytes of an artifact handle produced by `resourceId`. */
  artifactBytes(handle: string, resourceId: string): Promise<Uint8Array>
  /**
   * Release an artifact the runtime is holding for `resourceId`.
   *
   * The local runtime parks rendered bytes in a bounded session slot, so the
   * object model frees the slot as soon as the bytes have crossed. Runtimes
   * that hold nothing (the server addresses artifacts by content) omit this.
   */
  releaseArtifact?(handle: string, resourceId: string): Promise<void>
}

/** A canonical response envelope, before any operation-specific narrowing. */
export interface CanonicalEnvelope {
  schema_version: string
  operation: string
  resource_id?: string
  revision_id?: string
  data: unknown
}

/** True when `value` looks like a canonical response envelope. */
export function isCanonicalEnvelope(value: unknown): value is CanonicalEnvelope {
  if (!value || typeof value !== "object") return false
  const candidate = value as { schema_version?: unknown; operation?: unknown }
  return candidate.schema_version === "1" && typeof candidate.operation === "string"
}

/**
 * Execute a canonical operation with the capability gate applied first.
 *
 * An operation the live runtime does not advertise throws `CapabilityError` before
 * any bytes reach the transport.
 */
export async function executeCanonical<K extends OperationName>(
  runtime: CanonicalRuntime,
  operation: K,
  input: Record<string, unknown>,
  options: DispatchOptions = {}
): Promise<OutputOf<K>> {
  if (options.requestId !== undefined && (typeof options.requestId !== "string" ||
    new TextEncoder().encode(options.requestId).length < 1 ||
    new TextEncoder().encode(options.requestId).length > 256)) {
    throw new TypeError("requestId must be a string of 1–256 UTF-8 bytes")
  }
  if (!isOperationName(operation)) {
    throw new CapabilityError({
      capability: String(operation),
      operation: String(operation),
      message: `'${String(operation)}' is not a canonical operation in this SDK's registry`
    })
  }
  const available = await runtime.operations()
  if (!available.has(operation)) {
    throw new CapabilityError({
      capability: operation,
      operation,
      available: [...available],
      message: `the ${runtime.kind} runtime does not support '${operation}'`
    })
  }
  const response = await runtime.dispatch(operation, input, options)
  if (!isCanonicalEnvelope(response)) {
    throw new TransportError(
      `the ${runtime.kind} runtime returned a non-canonical response for '${operation}'`,
      { operation }
    )
  }
  return response
}

/**
 * The typed escape hatch. `execute` takes the full canonical input, including
 * `resource_id`; the object model injects resource ids for you.
 */
export class CanonicalApi {
  readonly #runtime: CanonicalRuntime

  constructor(runtime: CanonicalRuntime) {
    this.#runtime = runtime
  }

  /** Operation names this live runtime advertises. */
  async operations(): Promise<readonly string[]> {
    return [...(await this.#runtime.operations())]
  }

  /** Execute any canonical operation with its exact input and response types. */
  execute<K extends OperationName>(operation: K, input: InputOf<K>, options: DispatchOptions = {}): Promise<OutputOf<K>> {
    return executeCanonical(this.#runtime, operation, input as Record<string, unknown>, options)
  }
}
