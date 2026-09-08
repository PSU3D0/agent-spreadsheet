/**
 * `agent-spreadsheet-sdk` - one object model over two canonical runtimes.
 *
 * - {@link createLocalSpreadsheet} runs the operations in-process over the
 *   `agent-spreadsheet-wasm` bindings and owns workbook sessions.
 * - {@link connectSpreadsheetServer} drives an `agent-spreadsheet-mcp` process over the
 *   canonical `/v1` HTTP route and shares its workspace, forks, and checkpoints.
 *
 * The MCP client adapter was dropped in 0.15; MCP hosts use an MCP client. The 0.14
 * backends remain under `agent-spreadsheet-sdk/compat` for one release.
 */
export {
  CANONICAL_ERROR_STATUS,
  CanonicalOperationError,
  CapabilityError,
  SpreadsheetError,
  TransportError,
  canonicalEnvelope,
  decodeRejection,
  isCanonicalErrorEnvelope,
  statusForCanonicalCode
} from "./errors.js"

export {
  CANONICAL_SCHEMA_VERSION,
  OPERATION_NAMES,
  canonicalRegistry,
  descriptorFor,
  isOperationName,
  normalizeOperationList,
  operationsForAdapter
} from "./registry.js"
export type {
  CanonicalOperationDescriptor,
  CanonicalRegistryDocument
} from "./registry.js"

export { CanonicalApi, executeCanonical, isCanonicalEnvelope } from "./runtime.js"
export type { CanonicalEnvelope, CanonicalRuntime, DispatchOptions } from "./runtime.js"

export {
  CLIENT_SURFACE_OPERATIONS,
  GeneratedClientSurface,
  GeneratedWorkbookView,
  READ_SURFACE_OPERATIONS
} from "./generated/read-surface.js"
export type {
  BoundInput,
  ClientSurfaceOperation,
  OmitResource,
  ReadSurfaceOperation
} from "./generated/read-surface.js"

export type {
  CanonicalErrorCode,
  CanonicalErrorEnvelope,
  InputOf,
  OperationInputs,
  OperationName,
  OperationOutputs,
  OutputOf
} from "./generated/operations.js"

export { MutableWorkbookHandle, WorkbookHandle } from "./view.js"
export type { RecalculateInput, VerifyInput, WriteInput } from "./view.js"

export type {
  RenderFidelity,
  RenderPngLevel,
  RenderSheetInput,
  RenderWarning,
  RenderedSheet
} from "./render.js"

export { LocalSpreadsheet, LocalWorkbook, createLocalSpreadsheet } from "./local.js"
export type { LocalRuntimeSpec, LocalSpreadsheetOptions, WasmBindings } from "./local.js"

export {
  WORKER_METHODS,
  connectBindings,
  loadRuntimeModule,
  serveBindings,
  spawnWorkerBindings,
  workerSupported
} from "./worker.js"
export type {
  WorkerBindingsHandle,
  WorkerBindingsOptions,
  WorkerMethod,
  WorkerPortLike,
  WorkerRequest,
  WorkerResponse,
  WorkerRuntimeSpec
} from "./worker.js"

export {
  RemoteFork,
  RemoteWorkbook,
  ServerClient,
  connectSpreadsheetServer
} from "./server.js"
export type { FetchLike, ServerClientOptions } from "./server.js"
