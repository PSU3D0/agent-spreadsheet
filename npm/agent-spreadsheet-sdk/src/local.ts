import { CapabilityError, TransportError, decodeRejection } from "./errors.js"
import {
  type WorkerBindingsOptions,
  type WorkerRuntimeSpec,
  spawnWorkerBindings
} from "./worker.js"
import type { OperationName, OutputOf } from "./generated/operations.js"
import { GeneratedClientSurface } from "./generated/read-surface.js"
import { normalizeOperationList } from "./registry.js"
import { CanonicalApi, type CanonicalRuntime, executeCanonical } from "./runtime.js"
import { MutableWorkbookHandle, type VerifyInput } from "./view.js"

/**
 * The binding object exported by `agent-spreadsheet-wasm`
 * (`createWasmRuntime({ wasmUrl?, wasmBytes? })`).
 */
export interface WasmBindings {
  /** Create a resident session from workbook bytes; returns `session:<id>`. */
  createSession(bytes: Uint8Array): string | PromiseLike<string>
  /** Serialized canonical discovery for this runtime. */
  operations(): unknown | PromiseLike<unknown>
  /** Dispatch a canonical operation; rejects with a canonical error envelope string. */
  executeOperation(
    sessionId: string,
    operation: string,
    paramsJson: string,
    requestId?: string
  ): string | PromiseLike<string>
  /** Authoritative revision, durability and instrumentation from the Rust owner. */
  sessionMetadata?(sessionId: string): string | PromiseLike<string>
  /** Export the latest applied bytes for a session. */
  exportWorkbook?(sessionId: string): Uint8Array | PromiseLike<Uint8Array>
  /** Release a session. */
  disposeSession?(sessionId: string): unknown
  /**
   * Artifact bytes for a handle produced in this session.
   *
   * This is the boundary image bytes cross: the canonical envelope carries only
   * the handle. Rejects with a canonical error envelope, like `executeOperation`.
   */
  readArtifact?(sessionId: string, handle: string): Uint8Array | PromiseLike<Uint8Array>
  /** Release one artifact slot; `false` when the handle was already gone. */
  disposeArtifact?(sessionId: string, handle: string): boolean | PromiseLike<boolean>
}

/**
 * A runtime the SDK can instantiate itself, in this thread or in a worker.
 *
 * A live bindings object cannot be moved across a worker boundary, so worker
 * mode takes a spec instead: the module to import and the options to hand its
 * factory.
 */
export interface LocalRuntimeSpec extends WorkerRuntimeSpec {}

/** Options for {@link createLocalSpreadsheet}. */
export interface LocalSpreadsheetOptions {
  /** The bindings object, a promise of it, or a spec the SDK instantiates. */
  runtime: WasmBindings | PromiseLike<WasmBindings> | LocalRuntimeSpec
  /**
   * Run the bindings in a worker: a Web Worker in the browser, `worker_threads`
   * in Node. Rendering and recalculation are synchronous CPU work, so keeping
   * them off the UI thread is the point.
   *
   * Defaults to on in a browser when `Worker` exists and `runtime` is a spec
   * the SDK can move, and off in Node unless asked for. `true` throws when the
   * runtime cannot be moved, rather than silently blocking the caller's thread.
   */
  worker?: boolean | WorkerBindingsOptions
}

function assertBindings(bindings: unknown): asserts bindings is WasmBindings {
  const candidate = bindings as Partial<WasmBindings> | null
  if (!candidate || typeof candidate !== "object") {
    throw new TypeError("createLocalSpreadsheet requires a runtime bindings object")
  }
  // Byte export and session disposal are capability-gated rather than required: a
  // read-only build advertises fewer operations and still drives the read surface.
  for (const method of ["createSession", "operations", "executeOperation"] as const) {
    if (typeof candidate[method] !== "function") {
      throw new TypeError(`runtime bindings are missing ${method}()`)
    }
  }
}

class LocalRuntime implements CanonicalRuntime {
  readonly kind = "local"
  readonly bindings: WasmBindings
  #operations: ReadonlySet<string> | undefined

  constructor(bindings: WasmBindings) {
    this.bindings = bindings
  }

  async operations(): Promise<ReadonlySet<string>> {
    // Discovery is synchronous in the wasm-bindgen bindings and a promise
    // behind the worker shim; both are awaited so the runtime surface does not
    // depend on the transport.
    if (!this.#operations) {
      this.#operations = normalizeOperationList(await this.bindings.operations())
    }
    return this.#operations
  }

  async dispatch<K extends OperationName>(
    operation: K,
    input: Record<string, unknown>,
    options: { requestId?: string } = {}
  ): Promise<OutputOf<K>> {
    const sessionId = typeof input["resource_id"] === "string" ? (input["resource_id"] as string) : ""
    let raw: string
    try {
      raw = await this.bindings.executeOperation(sessionId, operation, JSON.stringify(input), options.requestId)
    } catch (rejection) {
      throw decodeRejection(rejection, { operation, runtime: this.kind })
    }
    return parseEnvelope(raw, operation)
  }

  async artifactBytes(handle: string, resourceId: string): Promise<Uint8Array> {
    if (typeof this.bindings.readArtifact !== "function") {
      throw new CapabilityError({
        capability: "readArtifact",
        message:
          "this WASM runtime cannot return artifact bytes; upgrade to a build with readArtifact()"
      })
    }
    try {
      return await this.bindings.readArtifact(resourceId, handle)
    } catch (rejection) {
      throw decodeRejection(rejection, { operation: "screenshot_sheet", runtime: this.kind })
    }
  }

  async releaseArtifact(handle: string, resourceId: string): Promise<void> {
    if (typeof this.bindings.disposeArtifact !== "function") return
    try {
      await this.bindings.disposeArtifact(resourceId, handle)
    } catch (rejection) {
      throw decodeRejection(rejection, { operation: "screenshot_sheet", runtime: this.kind })
    }
  }
}

function parseEnvelope<K extends OperationName>(raw: unknown, operation: K): OutputOf<K> {
  if (typeof raw !== "string") return raw as OutputOf<K>
  try {
    return JSON.parse(raw) as OutputOf<K>
  } catch (cause) {
    throw new TransportError(`the local runtime returned invalid JSON for '${operation}'`, {
      operation,
      cause
    })
  }
}

/** A workbook opened from bytes in the local WASM runtime. This handle owns its session. */
export class LocalWorkbook extends MutableWorkbookHandle {
  readonly #local: LocalRuntime
  #disposed = false
  #disposing: Promise<void> | undefined

  /** @internal */
  constructor(runtime: LocalRuntime, resourceId: string) {
    super(runtime, resourceId)
    this.#local = runtime
  }

  /** True once {@link dispose} has released the session. */
  get disposed(): boolean {
    return this.#disposed
  }

  protected override async executeBound<K extends OperationName>(
    operation: K,
    input: Record<string, unknown>
  ): Promise<OutputOf<K>> {
    this.#assertLive()
    return super.executeBound(operation, input)
  }

  /** Compare this workbook against another local workbook. */
  verifyAgainst(baseline: LocalWorkbook, input: VerifyInput = {}): Promise<OutputOf<"verify_workbook">> {
    return this.verifyAgainstResource(baseline.resourceId, input)
  }

  /**
   * Bytes for an artifact this session produced, released from the session on
   * the way out.
   *
   * Adapters that own a filesystem — the just-bash VFS, a CLI `--output` — use
   * this to land image bytes; the canonical envelope only ever carries the
   * handle.
   */
  async readArtifact(handle: string, options: { release?: boolean } = {}): Promise<Uint8Array> {
    this.#assertLive()
    const bytes = await this.#local.artifactBytes(handle, this.resourceId)
    if (options.release !== false) await this.#local.releaseArtifact(handle, this.resourceId)
    return bytes
  }

  /** Authoritative owner metadata; no adapter-maintained revision or counters. */
  async metadata(): Promise<unknown> {
    this.#assertLive()
    if (!this.#local.bindings.sessionMetadata) {
      throw new CapabilityError({ capability: "sessionMetadata", message: "runtime does not expose resident metadata" })
    }
    try {
      return JSON.parse(await this.#local.bindings.sessionMetadata(this.resourceId))
    } catch (rejection) {
      throw decodeRejection(rejection, { operation: "session_metadata", runtime: "local" })
    }
  }

  /** The latest applied or recalculated workbook bytes. */
  async exportBytes(): Promise<Uint8Array> {
    this.#assertLive()
    if (typeof this.#local.bindings.exportWorkbook !== "function") {
      throw new CapabilityError({
        capability: "exportWorkbook",
        message: "this WASM runtime cannot export workbook bytes"
      })
    }
    try {
      return await this.#local.bindings.exportWorkbook(this.resourceId)
    } catch (rejection) {
      throw decodeRejection(rejection, { operation: "export_workbook", runtime: "local" })
    }
  }

  /** Release the underlying WASM session. Safe to call twice. */
  dispose(): Promise<void> {
    if (this.#disposed) return Promise.resolve()
    if (this.#disposing) return this.#disposing
    const pending = (async () => {
      try {
        await this.#local.bindings.disposeSession?.(this.resourceId)
        this.#disposed = true
      } catch (rejection) {
        throw decodeRejection(rejection, { operation: "dispose_session", runtime: "local" })
      }
    })()
    this.#disposing = pending
    void pending.then(() => { this.#disposing = undefined }, () => { this.#disposing = undefined })
    return pending
  }

  /** `await using workbook = await local.open(bytes)`. */
  async [Symbol.asyncDispose](): Promise<void> {
    await this.dispose()
  }

  #assertLive(): void {
    if (this.#disposed || this.#disposing) {
      throw new CapabilityError({
        capability: "session",
        message: `local workbook ${this.resourceId} has been disposed`
      })
    }
  }
}

import { loadRuntimeModule } from "./worker.js"

function isBindings(value: unknown): value is WasmBindings {
  return typeof (value as WasmBindings | null)?.createSession === "function"
}

function isThenable(value: unknown): value is PromiseLike<WasmBindings> {
  return typeof (value as PromiseLike<WasmBindings> | null)?.then === "function"
}

function isRuntimeSpec(value: unknown): value is LocalRuntimeSpec {
  if (!value || typeof value !== "object" || isBindings(value) || isThenable(value)) return false
  return "module" in value || "options" in value
}

/** Instantiate a runtime spec in this thread. */
async function loadSpec(spec: LocalRuntimeSpec): Promise<WasmBindings> {
  const specifier = spec.module ?? "agent-spreadsheet-wasm"
  const loaded = await loadRuntimeModule(specifier)
  const factory = typeof loaded["createWasmRuntime"] === "function"
    ? loaded["createWasmRuntime"]
    : typeof loaded["default"] === "function" ? loaded["default"] : undefined
  if (factory) {
    return (await (factory as (options: unknown) => unknown)(spec.options ?? {})) as WasmBindings
  }
  const direct = isBindings(loaded) ? loaded : (loaded["default"] as unknown)
  if (!isBindings(direct)) {
    throw new TypeError(`'${specifier}' exports no agent-spreadsheet runtime bindings`)
  }
  return direct
}

/** The local (WASM) client. Workbooks are opened from bytes and own their session. */
export class LocalSpreadsheet extends GeneratedClientSurface {
  readonly #options: LocalSpreadsheetOptions
  #runtime: Promise<LocalRuntime> | undefined
  #terminate: (() => Promise<void>) | undefined
  #canonical: CanonicalApi | undefined

  /** @internal */
  constructor(options: LocalSpreadsheetOptions) {
    super()
    if (!options || options.runtime === undefined) {
      throw new TypeError("createLocalSpreadsheet requires { runtime }")
    }
    this.#options = options
  }

  /** True when the bindings run behind the worker shim. */
  get worker(): boolean {
    return this.#terminate !== undefined
  }

  /** Shut the worker down, when this client started one. Safe to call twice. */
  async close(): Promise<void> {
    const terminate = this.#terminate
    this.#terminate = undefined
    if (terminate) await terminate()
  }

  /** The typed canonical escape hatch. Inputs carry their own `resource_id`. */
  get canonical(): CanonicalApi {
    if (!this.#canonical) this.#canonical = new CanonicalApi(lazyRuntime(() => this.#resolve()))
    return this.#canonical
  }

  /** Operation names the live WASM runtime advertises. */
  async capabilities(): Promise<readonly string[]> {
    const runtime = await this.#resolve()
    return [...(await runtime.operations())]
  }

  /** Open workbook bytes as a resident session. */
  async open(bytes: Uint8Array): Promise<LocalWorkbook> {
    if (!(bytes instanceof Uint8Array)) {
      throw new TypeError("local.open requires workbook bytes as a Uint8Array")
    }
    const runtime = await this.#resolve()
    let resourceId: string
    try {
      resourceId = await runtime.bindings.createSession(bytes)
    } catch (rejection) {
      throw decodeRejection(rejection, { operation: "create_session", runtime: "local" })
    }
    if (typeof resourceId !== "string" || resourceId.length === 0) {
      throw new TransportError("the local runtime returned an empty session id")
    }
    return new LocalWorkbook(runtime, resourceId)
  }

  protected override async executeClient<K extends OperationName>(
    operation: K,
    input: Record<string, unknown>
  ): Promise<OutputOf<K>> {
    return executeCanonical(await this.#resolve(), operation, input)
  }

  async #resolve(): Promise<LocalRuntime> {
    if (!this.#runtime) {
      this.#runtime = this.#bind().then((bindings) => {
        assertBindings(bindings)
        return new LocalRuntime(bindings)
      })
    }
    return this.#runtime
  }

  async #bind(): Promise<WasmBindings> {
    const runtime = this.#options.runtime
    const requested = this.#options.worker
    // A spec is recognised structurally, and narrowly: anything else is treated
    // as bindings so a malformed object still fails the bindings assertion
    // rather than silently importing a package the caller never named.
    const spec: LocalRuntimeSpec | undefined = isRuntimeSpec(runtime) ? runtime : undefined
    const explicit = requested !== undefined && requested !== false
    const workerOptions: WorkerBindingsOptions =
      typeof requested === "object" && requested !== null ? requested : {}
    const movable = spec !== undefined || workerOptions.port !== undefined
    // A browser gets the worker by default; blocking the UI thread with a
    // synchronous render is exactly what this mode exists to avoid.
    const preferred = requested === undefined &&
      typeof (globalThis as { Worker?: unknown }).Worker === "function" &&
      movable

    if (explicit && !movable) {
      throw new TypeError(
        "worker mode needs { runtime: { module, options } } or { worker: { port } }; " +
        "a live bindings object cannot cross a worker boundary"
      )
    }
    if (explicit || preferred) {
      const handle = await spawnWorkerBindings({ ...spec, ...workerOptions })
      this.#terminate = handle.terminate
      return handle.bindings
    }
    if (spec) return loadSpec(spec)
    return Promise.resolve(runtime as WasmBindings | PromiseLike<WasmBindings>)
  }
}

function lazyRuntime(resolve: () => Promise<CanonicalRuntime>): CanonicalRuntime {
  return {
    kind: "local",
    async operations() {
      return (await resolve()).operations()
    },
    async dispatch(operation, input, options) {
      return (await resolve()).dispatch(operation, input, options)
    },
    async artifactBytes(handle, resourceId) {
      return (await resolve()).artifactBytes(handle, resourceId)
    },
    async releaseArtifact(handle, resourceId) {
      const runtime = await resolve()
      if (runtime.releaseArtifact) await runtime.releaseArtifact(handle, resourceId)
    }
  }
}

/** Create a local spreadsheet client over the `agent-spreadsheet-wasm` bindings. */
export function createLocalSpreadsheet(options: LocalSpreadsheetOptions): LocalSpreadsheet {
  return new LocalSpreadsheet(options)
}
