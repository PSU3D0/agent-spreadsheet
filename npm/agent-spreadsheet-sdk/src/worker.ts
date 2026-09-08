/**
 * Worker mode for the local runtime.
 *
 * Rendering and recalculation are synchronous CPU work. In a browser that work
 * must not sit on the UI thread, so the bindings can run inside a Web Worker
 * (or `worker_threads` in Node) behind a small request/response shim while the
 * main thread keeps the same {@link WasmBindings} surface.
 *
 * The protocol is deliberately tiny: one message per call, correlated by id,
 * carrying either the resolved value or the canonical error envelope the
 * binding rejected with. Envelopes survive `structuredClone`, so
 * `decodeRejection` still produces a `CanonicalOperationError` on the far side.
 */

import { TransportError } from "./errors.js"
import type { WasmBindings } from "./local.js"

/** Methods the shim forwards. Everything else stays on the main thread. */
export const WORKER_METHODS = [
  "createSession",
  "operations",
  "executeOperation",
  "sessionMetadata",
  "exportWorkbook",
  "disposeSession",
  "readArtifact",
  "disposeArtifact"
] as const

/** A method name the worker shim forwards. */
export type WorkerMethod = (typeof WORKER_METHODS)[number]

/** The subset of `Worker`/`MessagePort` the shim uses, in either runtime. */
export interface WorkerPortLike {
  postMessage(message: unknown): void
  addEventListener?(type: "message" | "error" | "messageerror" | "exit" | "close", listener: (event: unknown) => void): void
  on?(type: "message" | "error" | "messageerror" | "exit" | "close", listener: (value: unknown) => void): void
  terminate?(): unknown
  unref?(): unknown
}

/** A request the main thread sends to the worker. */
export interface WorkerRequest {
  asp: 1
  id: number
  method: WorkerMethod
  args: unknown[]
}

/** The worker's reply. `error` carries a canonical envelope when there is one. */
export interface WorkerResponse {
  asp: 1
  id: number
  ok: boolean
  value?: unknown
  error?: unknown
}

/** How to obtain the bindings the worker will serve. */
export interface WorkerRuntimeSpec {
  /**
   * Module specifier the worker imports. Defaults to `agent-spreadsheet-wasm`.
   * The module may export `createWasmRuntime`, a default factory, or be the
   * wasm-bindgen bindings themselves.
   */
  module?: string
  /** Options forwarded to the module's factory; must be structured-cloneable. */
  options?: Record<string, unknown>
}

/** Options for {@link spawnWorkerBindings}. */
export interface WorkerBindingsOptions extends WorkerRuntimeSpec {
  /** An already-created `Worker` or `MessagePort` to talk to. */
  port?: WorkerPortLike
  /** Module URL of a worker entry that calls {@link serveBindings}. */
  url?: string | URL
}

/** A worker-backed bindings object plus the handle that shuts the worker down. */
export interface WorkerBindingsHandle {
  bindings: WasmBindings
  terminate(): Promise<void>
}

function listen(port: WorkerPortLike, type: "message" | "error" | "messageerror" | "exit" | "close", handler: (value: unknown) => void): void {
  if (typeof port.on === "function") {
    // worker_threads delivers the payload itself.
    port.on(type, handler)
    return
  }
  if (typeof port.addEventListener === "function") {
    port.addEventListener(type, (event: unknown) => {
      const detail = event as { data?: unknown; message?: unknown }
      handler(type === "message" ? detail?.data : (detail?.message ?? event))
    })
    return
  }
  throw new TypeError("worker port exposes neither on() nor addEventListener()")
}

function isRequest(value: unknown): value is WorkerRequest {
  const candidate = value as WorkerRequest | null
  return Boolean(candidate) && candidate?.asp === 1 && typeof candidate.id === "number" &&
    typeof candidate.method === "string"
}

function isResponse(value: unknown): value is WorkerResponse {
  const candidate = value as WorkerResponse | null
  return Boolean(candidate) && candidate?.asp === 1 && typeof candidate.id === "number" &&
    typeof candidate.ok === "boolean"
}

/**
 * Serve `bindings` on `port`. Call this inside the worker.
 *
 * A rejection is forwarded as-is when it is already a canonical envelope, and
 * flattened to `{ message }` otherwise, because `Error` instances do not always
 * survive structured clone with their prototype intact.
 */
export function serveBindings(
  port: WorkerPortLike,
  bindings: WasmBindings | PromiseLike<WasmBindings>
): void {
  const resolved = Promise.resolve(bindings)
  listen(port, "message", (message) => {
    if (!isRequest(message)) return
    const { id, method, args } = message
    void (async () => {
      try {
        const target = (await resolved) as unknown as Record<string, unknown>
        const fn = target[method]
        if (typeof fn !== "function") {
          throw new TypeError(`runtime bindings are missing ${method}()`)
        }
        const value = await (fn as (...rest: unknown[]) => unknown).apply(target, args)
        port.postMessage({ asp: 1, id, ok: true, value } satisfies WorkerResponse)
      } catch (rejection) {
        port.postMessage({
          asp: 1,
          id,
          ok: false,
          error: rejection instanceof Error ? { message: rejection.message } : rejection
        } satisfies WorkerResponse)
      }
    })()
  })
}

/** A live channel: the forwarding bindings plus the abort hook shutdown uses. */
interface Channel {
  bindings: WasmBindings
  abort(reason: unknown): void
}

/**
 * The main-thread half: a {@link WasmBindings} object that forwards every call
 * over `port`.
 */
export function connectBindings(port: WorkerPortLike): WasmBindings {
  return createChannel(port).bindings
}

function createChannel(port: WorkerPortLike): Channel {
  const pending = new Map<number, { resolve: (value: unknown) => void; reject: (reason: unknown) => void }>()
  let sequence = 0
  let failure: TransportError | undefined

  listen(port, "message", (message) => {
    if (!isResponse(message)) return
    const settle = pending.get(message.id)
    if (!settle) return
    pending.delete(message.id)
    if (message.ok) settle.resolve(message.value)
    else settle.reject(message.error)
  })
  function abort(reason: unknown): void {
    if (failure) return
    failure = reason instanceof TransportError ? reason : new TransportError("the local runtime worker connection closed or failed; operation completion is unknown", { cause: reason })
    for (const [id, settle] of pending) {
      pending.delete(id)
      settle.reject(failure)
    }
  }
  listen(port, "error", abort)
  listen(port, "messageerror", abort)
  listen(port, "close", abort)
  if (typeof port.on === "function") listen(port, "exit", abort)

  function call(method: WorkerMethod, args: unknown[]): Promise<unknown> {
    if (failure !== undefined) {
      return Promise.reject(
        new TransportError("the local runtime worker failed", { cause: failure })
      )
    }
    const id = ++sequence
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject })
      try {
        port.postMessage({ asp: 1, id, method, args } satisfies WorkerRequest)
      } catch (cause) {
        pending.delete(id)
        reject(new TransportError(`could not post '${method}' to the local runtime worker`, { cause }))
      }
    })
  }

  const bindings: WasmBindings = {
    createSession: (bytes: Uint8Array) => call("createSession", [bytes]) as Promise<string>,
    operations: () => call("operations", []),
    sessionMetadata: (sessionId: string) => call("sessionMetadata", [sessionId]) as Promise<string>,
    executeOperation: (sessionId: string, operation: string, paramsJson: string, requestId?: string) =>
      call("executeOperation", [sessionId, operation, paramsJson, requestId]) as Promise<string>,
    exportWorkbook: (sessionId: string) =>
      call("exportWorkbook", [sessionId]) as Promise<Uint8Array>,
    disposeSession: (sessionId: string) => call("disposeSession", [sessionId]),
    readArtifact: (sessionId: string, handle: string) =>
      call("readArtifact", [sessionId, handle]) as Promise<Uint8Array>,
    disposeArtifact: (sessionId: string, handle: string) =>
      call("disposeArtifact", [sessionId, handle]) as Promise<boolean>
  }
  return { bindings, abort }
}

// The Node worker body. It is inlined rather than shipped as a file so the same
// source works from dist/cjs and dist/esm without either format having to know
// its own path on disk.
const NODE_WORKER_SOURCE = `
const { parentPort, workerData } = require("node:worker_threads")
async function loadModule(specifier) {
  try {
    return require(specifier)
  } catch (error) {
    return await import(specifier)
  }
}
async function bindingsFor(spec) {
  const module = await loadModule(spec.module)
  const candidate = typeof module.createWasmRuntime === "function"
    ? module.createWasmRuntime
    : typeof module.default === "function" ? module.default : null
  if (candidate) return await candidate(spec.options || {})
  const direct = typeof module.createSession === "function" ? module : module.default
  if (!direct || typeof direct.createSession !== "function") {
    throw new TypeError("worker runtime module exports no bindings")
  }
  return direct
}
const ready = bindingsFor(workerData)
parentPort.on("message", async (message) => {
  if (!message || message.asp !== 1 || typeof message.id !== "number") return
  try {
    const bindings = await ready
    const fn = bindings[message.method]
    if (typeof fn !== "function") throw new TypeError("runtime bindings are missing " + message.method + "()")
    const value = await fn.apply(bindings, message.args)
    parentPort.postMessage({ asp: 1, id: message.id, ok: true, value })
  } catch (rejection) {
    parentPort.postMessage({
      asp: 1,
      id: message.id,
      ok: false,
      error: rejection instanceof Error ? { message: rejection.message } : rejection
    })
  }
})
`

/**
 * A real dynamic `import()` in both build formats.
 *
 * The CJS build downlevels a literal `import()` to `require()`, which cannot
 * load the ESM-only WASM loader package, so the call is hidden from the
 * compiler. Only spec-based and worker runtimes take this path; passing a live
 * bindings object never does.
 */
export const dynamicImport: (specifier: string) => Promise<Record<string, unknown>> =
  new Function("specifier", "return import(specifier)") as (
    specifier: string
  ) => Promise<Record<string, unknown>>

/**
 * Import a runtime module by specifier, in either module format.
 *
 * A wasm-pack `--target nodejs` package is CommonJS and a directory path, which
 * ESM resolution rejects; the loader package is ESM-only, which `require`
 * historically rejected. Try the ESM path first and fall back to Node's
 * resolver.
 */
export async function loadRuntimeModule(specifier: string): Promise<Record<string, unknown>> {
  try {
    return await dynamicImport(specifier)
  } catch (error) {
    if (!isNode()) throw error
    const { createRequire } = (await dynamicImport("node:module")) as unknown as {
      createRequire: (base: string) => (specifier: string) => Record<string, unknown>
    }
    const cwd = (globalThis as { process?: { cwd(): string } }).process?.cwd() ?? "."
    return createRequire(`${cwd}/`)(specifier)
  }
}

function isNode(): boolean {
  const candidate = globalThis as { process?: { versions?: { node?: string } } }
  return typeof candidate.process?.versions?.node === "string"
}

/** True when this runtime can host a worker for the local bindings. */
export function workerSupported(): boolean {
  return typeof (globalThis as { Worker?: unknown }).Worker === "function" || isNode()
}

/**
 * Start a worker that serves the bindings, and return the main-thread shim.
 *
 * Resolution order: an explicit `port`, then an explicit worker `url`, then a
 * Node `worker_threads` worker that imports `module` itself.
 */
export async function spawnWorkerBindings(
  options: WorkerBindingsOptions = {}
): Promise<WorkerBindingsHandle> {
  const spec = {
    module: options.module ?? "agent-spreadsheet-wasm",
    options: options.options ?? {}
  }

  if (options.port) {
    const port = options.port
    const channel = createChannel(port)
    return {
      bindings: channel.bindings,
      terminate: async () => {
        channel.abort(new TransportError("the local runtime worker was terminated"))
        port.terminate?.()
      }
    }
  }

  const BrowserWorker = (globalThis as { Worker?: new (url: string | URL, opts?: unknown) => WorkerPortLike }).Worker
  if (options.url && typeof BrowserWorker === "function") {
    const worker = new BrowserWorker(options.url, { type: "module" })
    worker.postMessage({ asp: 1, id: 0, method: "init", args: [spec] })
    const channel = createChannel(worker)
    return {
      bindings: channel.bindings,
      terminate: async () => {
        channel.abort(new TransportError("the local runtime worker was terminated"))
        worker.terminate?.()
      }
    }
  }

  if (isNode()) {
    const { Worker } = (await dynamicImport("node:worker_threads")) as unknown as {
      Worker: new (source: string | URL, opts?: Record<string, unknown>) => WorkerPortLike
    }
    // `execArgv: []` matters: a worker inherits the parent's flags, and a
    // process running under `node --test` would otherwise start the test runner
    // inside the worker instead of the shim.
    const worker = options.url
      ? new Worker(options.url, { workerData: spec, execArgv: [] })
      : new Worker(NODE_WORKER_SOURCE, { eval: true, workerData: spec, execArgv: [] })
    worker.unref?.()
    const channel = createChannel(worker)
    return {
      bindings: channel.bindings,
      terminate: async () => {
        channel.abort(new TransportError("the local runtime worker was terminated"))
        await worker.terminate?.()
      }
    }
  }

  throw new TransportError(
    "worker mode needs a Web Worker with { url }, an explicit { port }, or a Node runtime"
  )
}
