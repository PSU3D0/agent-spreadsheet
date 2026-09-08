import {
  CanonicalOperationError,
  TransportError,
  isCanonicalErrorEnvelope
} from "./errors.js"
import type { InputOf, OperationName, OutputOf } from "./generated/operations.js"
import { GeneratedClientSurface } from "./generated/read-surface.js"
import { normalizeOperationList } from "./registry.js"
import { CanonicalApi, type CanonicalRuntime, executeCanonical } from "./runtime.js"
import { MutableWorkbookHandle, WorkbookHandle, type VerifyInput } from "./view.js"

/** Minimal `fetch` shape this SDK depends on. */
export type FetchLike = (input: string, init?: {
  method?: string
  headers?: Record<string, string>
  body?: string
}) => Promise<{
  ok: boolean
  status: number
  text(): Promise<string>
  arrayBuffer(): Promise<ArrayBuffer>
}>

/** Options for {@link connectSpreadsheetServer}. */
export interface ServerClientOptions {
  /** Server root, for example `http://127.0.0.1:8079`. A trailing `/v1` is accepted. */
  baseUrl: string
  /** Override the global `fetch`. */
  fetch?: FetchLike
  /** Extra request headers, for example an authenticating proxy's token. */
  headers?: Record<string, string>
}

function canonicalRoot(baseUrl: string): string {
  const trimmed = String(baseUrl).replace(/\/+$/, "")
  if (trimmed.length === 0) throw new TypeError("connectSpreadsheetServer requires a baseUrl")
  return trimmed.endsWith("/v1") ? trimmed : `${trimmed}/v1`
}

const MAX_ERROR_BODY = 2048

class ServerRuntime implements CanonicalRuntime {
  readonly kind = "server"
  readonly #root: string
  readonly #fetch: FetchLike
  readonly #headers: Record<string, string>
  #operations: Promise<ReadonlySet<string>> | undefined

  constructor(options: ServerClientOptions) {
    this.#root = canonicalRoot(options.baseUrl)
    const provided = options.fetch ?? (globalThis as { fetch?: FetchLike }).fetch
    if (typeof provided !== "function") {
      throw new TypeError("connectSpreadsheetServer requires fetch; pass { fetch } on Node 16")
    }
    this.#fetch = provided
    this.#headers = { ...(options.headers ?? {}) }
  }

  /** The `/v1` root this client talks to. */
  get root(): string {
    return this.#root
  }

  operations(): Promise<ReadonlySet<string>> {
    if (!this.#operations) {
      this.#operations = this.#get("/operations")
        .then((body) => normalizeOperationList(JSON.parse(body)))
        .catch((error: unknown) => {
          this.#operations = undefined
          throw error
        })
    }
    return this.#operations
  }

  /** Drop the cached capability list so the next call re-reads `GET /v1/operations`. */
  refresh(): void {
    this.#operations = undefined
  }

  async dispatch<K extends OperationName>(
    operation: K,
    input: Record<string, unknown>,
    options: { requestId?: string } = {}
  ): Promise<OutputOf<K>> {
    const url = `${this.#root}/op/${encodeURIComponent(operation)}`
    let response: Awaited<ReturnType<FetchLike>>
    try {
      response = await this.#fetch(url, {
        method: "POST",
        headers: { "content-type": "application/json", ...this.#headers,
          ...(options.requestId === undefined ? {} : { "X-Agent-Spreadsheet-Request-Id": options.requestId }) },
        body: JSON.stringify(input)
      })
    } catch (cause) {
      throw new TransportError(`POST ${url} failed`, { operation, cause })
    }
    const body = await response.text()
    if (!response.ok) throw this.#failure(response.status, body, operation)
    let parsed: unknown
    try {
      parsed = JSON.parse(body)
    } catch (cause) {
      throw new TransportError(`the server returned invalid JSON for '${operation}'`, {
        status: response.status,
        body: body.slice(0, MAX_ERROR_BODY),
        operation,
        cause
      })
    }
    if (isCanonicalErrorEnvelope(parsed)) {
      throw new CanonicalOperationError(parsed, {
        runtime: this.kind,
        status: response.status,
        operation
      })
    }
    return parsed as OutputOf<K>
  }

  async artifactBytes(handle: string): Promise<Uint8Array> {
    const url = `${this.#root}/artifacts/${encodeURIComponent(handle)}`
    let response: Awaited<ReturnType<FetchLike>>
    try {
      response = await this.#fetch(url, { headers: { ...this.#headers } })
    } catch (cause) {
      throw new TransportError(`GET ${url} failed`, { operation: "screenshot_sheet", cause })
    }
    if (!response.ok) {
      throw this.#failure(response.status, await response.text(), "screenshot_sheet")
    }
    return new Uint8Array(await response.arrayBuffer())
  }

  async #get(path: string): Promise<string> {
    const url = `${this.#root}${path}`
    let response: Awaited<ReturnType<FetchLike>>
    try {
      response = await this.#fetch(url, { headers: { ...this.#headers } })
    } catch (cause) {
      throw new TransportError(`GET ${url} failed`, { cause })
    }
    const body = await response.text()
    if (!response.ok) throw this.#failure(response.status, body)
    return body
  }

  /** Project an HTTP failure onto the canonical hierarchy using the pinned status table. */
  #failure(status: number, body: string, operation?: string): TransportError | CanonicalOperationError {
    let parsed: unknown
    try {
      parsed = JSON.parse(body)
    } catch {
      parsed = undefined
    }
    if (isCanonicalErrorEnvelope(parsed)) {
      return new CanonicalOperationError(parsed, { runtime: this.kind, status, operation })
    }
    return new TransportError(`the server answered ${status} with a non-canonical body`, {
      status,
      body: body.slice(0, MAX_ERROR_BODY),
      operation
    })
  }
}

/** A read-only handle on a server-side workbook. It owns nothing and is never disposed. */
export class RemoteWorkbook extends WorkbookHandle {
  /** @internal */
  constructor(runtime: CanonicalRuntime, resourceId: string, revisionId?: string) {
    super(runtime, resourceId, revisionId)
  }

  /** Cheap exact workbook metadata. */
  describe(
    input: Parameters<RemoteWorkbook["describeWorkbook"]>[0] = {}
  ): Promise<OutputOf<"describe_workbook">> {
    return this.describeWorkbook(input)
  }

  /** Create a writable fork. The tracked revision is used unless one is supplied. */
  async createFork(input: { expected_revision?: string } = {}): Promise<RemoteFork> {
    const expected = input.expected_revision ?? (await this.currentRevision())
    const response = await this.executeCreateFork(expected)
    const forkId = (response as { resource_id?: string }).resource_id
    if (typeof forkId !== "string") {
      throw new TransportError("create_fork returned no fork resource id", {
        operation: "create_fork"
      })
    }
    return new RemoteFork(this.runtime, forkId, (response as { revision_id?: string }).revision_id)
  }

  private executeCreateFork(expectedRevision: string): Promise<OutputOf<"create_fork">> {
    // create_fork answers with the fork's identity, so it must not update this handle.
    return executeCanonical(this.runtime, "create_fork", {
      resource_id: this.resourceId,
      expected_revision: expectedRevision
    })
  }
}

/** A writable server-side fork. Disposing it discards the fork. */
export class RemoteFork extends MutableWorkbookHandle {
  #discarded = false

  /** @internal */
  constructor(runtime: CanonicalRuntime, resourceId: string, revisionId?: string) {
    super(runtime, resourceId, revisionId)
  }

  /** True once the fork has been discarded. */
  get discarded(): boolean {
    return this.#discarded
  }

  /** Compare this fork against another server workbook or fork. */
  verifyAgainst(
    baseline: RemoteWorkbook | RemoteFork,
    input: VerifyInput = {}
  ): Promise<OutputOf<"verify_workbook">> {
    return this.verifyAgainstResource(baseline.resourceId, input)
  }

  /** Structured diff of this fork against its source. */
  getChanges(
    input: Omit<InputOf<"get_changes">, "resource_id">
  ): Promise<OutputOf<"get_changes">> {
    return this.executeBound("get_changes", input as Record<string, unknown>)
  }

  /** Create, list, restore, or delete checkpoints. The revision defaults to the tracked one. */
  async checkpoint(
    input: Omit<InputOf<"checkpoint">, "resource_id">
  ): Promise<OutputOf<"checkpoint">> {
    return this.executeBound("checkpoint", await this.withRevision(input))
  }

  /** List, apply, or discard durable staged changes. */
  async stagedChange(
    input: Omit<InputOf<"staged_change">, "resource_id">
  ): Promise<OutputOf<"staged_change">> {
    return this.executeBound("staged_change", await this.withRevision(input))
  }

  /** Export the fork to a workspace destination. */
  async exportFork(
    input: Omit<InputOf<"export_fork">, "resource_id" | "expected_revision"> & {
      expected_revision?: string
    }
  ): Promise<OutputOf<"export_fork">> {
    const expected = input.expected_revision ?? (await this.currentRevision())
    return this.executeBound("export_fork", { ...input, expected_revision: expected })
  }

  /** Discard the fork. Safe to call twice. */
  async discard(input: { expected_revision?: string } = {}): Promise<OutputOf<"discard_fork"> | undefined> {
    if (this.#discarded) return undefined
    const expected = input.expected_revision ?? (await this.currentRevision())
    const response = await this.executeBound("discard_fork", { expected_revision: expected })
    this.#discarded = true
    return response
  }

  /** `await using fork = await workbook.createFork()` discards on scope exit. */
  async [Symbol.asyncDispose](): Promise<void> {
    await this.discard()
  }

  private async withRevision(
    input: Record<string, unknown> | object
  ): Promise<Record<string, unknown>> {
    const request = { ...(input as Record<string, unknown>) }
    const action = request["action"]
    const needsRevision = action !== "list" && request["expected_revision"] === undefined
    if (needsRevision) request["expected_revision"] = await this.currentRevision()
    return request
  }
}

/** The server client: one process, one workspace, many workbooks. */
export class ServerClient extends GeneratedClientSurface {
  readonly #runtime: ServerRuntime
  readonly #canonical: CanonicalApi

  /** @internal */
  constructor(options: ServerClientOptions) {
    super()
    this.#runtime = new ServerRuntime(options)
    this.#canonical = new CanonicalApi(this.#runtime)
  }

  /** The `/v1` root this client talks to. */
  get baseUrl(): string {
    return this.#runtime.root
  }

  /** The typed canonical escape hatch. Inputs carry their own `resource_id`. */
  get canonical(): CanonicalApi {
    return this.#canonical
  }

  /** Operation names the live server advertises through `GET /v1/operations`. */
  async capabilities(): Promise<readonly string[]> {
    return [...(await this.#runtime.operations())]
  }

  /** Re-read `GET /v1/operations` on the next capability check. */
  refresh(): void {
    this.#runtime.refresh()
  }

  /**
   * Durable forks this process is holding.
   *
   * `list_forks` takes no resource and is client-level in practice, but the registry
   * classifies it as durable orchestration, so it is written here rather than
   * generated into the shared client surface.
   */
  listForks(input: InputOf<"list_forks"> = {}): Promise<OutputOf<"list_forks">> {
    return executeCanonical(this.#runtime, "list_forks", input as Record<string, unknown>)
  }

  /** A non-owning read handle on a workspace workbook. */
  workbook(resourceId: string, revisionId?: string): RemoteWorkbook {
    if (typeof resourceId !== "string" || resourceId.length === 0) {
      throw new TypeError("client.workbook requires a canonical resource id")
    }
    return new RemoteWorkbook(this.#runtime, resourceId, revisionId)
  }

  protected override executeClient<K extends OperationName>(
    operation: K,
    input: Record<string, unknown>
  ): Promise<OutputOf<K>> {
    return executeCanonical(this.#runtime, operation, input)
  }
}

/** Connect to an `agent-spreadsheet-mcp` process serving the canonical `/v1` route. */
export function connectSpreadsheetServer(options: ServerClientOptions): ServerClient {
  return new ServerClient(options)
}
