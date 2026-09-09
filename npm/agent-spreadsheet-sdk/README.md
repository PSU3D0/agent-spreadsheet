# agent-spreadsheet-sdk

A TypeScript SDK for driving spreadsheets through the agent-spreadsheet canonical
operation protocol. One object model, two runtimes:

- **local** — the operations run in-process on WebAssembly. You own workbook bytes.
- **server** — the operations run in an `agent-spreadsheet-mcp` process over its
  canonical `/v1` HTTP route, sharing that process's workspace, forks, and checkpoints.

```bash
npm install agent-spreadsheet-sdk agent-spreadsheet-wasm   # local runtime
npm install agent-spreadsheet-sdk                          # server runtime only
```

Node.js 18 or newer. Ships CommonJS, ESM, and declarations; no runtime dependencies.

Every JavaScript block in this file is executed by `test/readme.test.js`.

## Local runtime

`local.open(bytes)` returns a `LocalWorkbook` that owns a WASM session and tracks its
own `resource_id` and `revision_id`. Compare-and-swap revisions are filled in for you.

```js
const fs = require("node:fs")
const { createWasmRuntime } = require("agent-spreadsheet-wasm")
const { createLocalSpreadsheet } = require("agent-spreadsheet-sdk")

const local = createLocalSpreadsheet({ runtime: createWasmRuntime({}) })
const workbook = await local.open(fs.readFileSync("book.xlsx"))

const sheets = await workbook.listSheets()
console.log(workbook.resourceId, sheets.operation)

// expected_revision defaults to the revision this workbook last saw.
const written = await workbook.write({
  mode: "apply",
  ops: [{
    kind: "set_cells",
    sheet_name: "Sheet1",
    cells: { A1: { kind: "value", value: "e" } }
  }]
})
console.log(written.data.status, workbook.revisionId)

await workbook.recalculate({ backend: "formualizer" })
fs.writeFileSync("book.out.xlsx", await workbook.exportBytes())
await workbook.dispose()
```

`dispose()` releases the session. `LocalWorkbook` also implements `Symbol.asyncDispose`,
so `await using workbook = await local.open(bytes)` releases it at the end of the scope.

Verification binds two sessions in one call:

```js
const fs = require("node:fs")
const { createWasmRuntime } = require("agent-spreadsheet-wasm")
const { createLocalSpreadsheet } = require("agent-spreadsheet-sdk")

const local = createLocalSpreadsheet({ runtime: createWasmRuntime({}) })
const bytes = fs.readFileSync("book.xlsx")
const current = await local.open(bytes)
const baseline = await local.open(bytes)

const proof = await current.verifyAgainst(baseline, {
  targets: ["Sheet1!A1"],
  targets_only: true
})
console.log(proof.operation)

await current.dispose()
await baseline.dispose()
```

## Server runtime

`connectSpreadsheetServer` speaks the canonical `/v1` route of a running
`agent-spreadsheet-mcp` process. Reads go straight to a workbook; writes go to a fork.

```js
const { connectSpreadsheetServer } = require("agent-spreadsheet-sdk")

const client = connectSpreadsheetServer({ baseUrl: "http://127.0.0.1:8079" })

const listed = await client.listWorkbooks({ limit: 10 })
const workbook = client.workbook(listed.data.workbooks[0].resource_id)

const described = await workbook.describe()
console.log(described.resource_id, described.revision_id)

const fork = await workbook.createFork()
await fork.write({
  mode: "apply",
  ops: [{
    kind: "set_cells",
    sheet_name: "Sheet1",
    cells: { A1: { kind: "value", value: "e" } }
  }]
})
await fork.checkpoint({ action: "create", label: "after-write" })

const proof = await fork.verifyAgainst(workbook, { targets_only: true })
console.log(proof.operation)

await fork.discard()
```

`RemoteWorkbook` is a non-owning read handle: it never disposes anything. `RemoteFork`
adds `write`, `recalculate`, `verifyAgainst`, `getChanges`, `checkpoint`, `stagedChange`,
`exportFork`, and `discard`, and its `Symbol.asyncDispose` discards the fork.

The `/v1` route has no authentication; its loopback bind is the security boundary. Pass
`fetch` and `headers` if you front it with an authenticating proxy:

```js
const { connectSpreadsheetServer } = require("agent-spreadsheet-sdk")

const client = connectSpreadsheetServer({
  baseUrl: "http://127.0.0.1:8079",
  headers: { "x-proxy-token": "local-only" },
  fetch: (url, init) => fetch(url, init)
})
console.log((await client.capabilities()).includes("read_cells"))
```

## The shared read surface

Every workbook-shaped object — `LocalWorkbook`, `RemoteWorkbook`, `RemoteFork` — exposes
the same generated read surface, one method per single-resource read operation
(`describeWorkbook`, `listSheets`, `sheetOverview`, `readCells`, `inspectCells`,
`readTable`, `readLayout`, `exportGrid`, `namedRanges`, `analyzeStyles`, `searchValues`,
`searchFormulas`, `formulaTrace`, `formulaMap`, `profileTable`, `sheetStatistics`,
`screenshotSheet`, `sheetportManifest`, `executeSheetport`, `inspectVba`). The methods are
generated as real declarations from the registry, so editors complete them and their
inputs and outputs are typed. Responses are canonical envelopes, unmodified.

```js
const { connectSpreadsheetServer, READ_SURFACE_OPERATIONS } = require("agent-spreadsheet-sdk")

const client = connectSpreadsheetServer({ baseUrl: "http://127.0.0.1:8079" })
const workbook = client.workbook("wb:wb-1")

const cells = await workbook.readCells({
  sheet_name: "Sheet1",
  selection: { kind: "range", ranges: ["A1:B2"] },
  format: "dense"
})
console.log(cells.operation, READ_SURFACE_OPERATIONS.includes("read_cells"))
```

`resource_id` is injected by the object, never by you. When you do want the raw protocol,
`client.canonical.execute` is the typed escape hatch and takes the full canonical input:

```js
const { connectSpreadsheetServer } = require("agent-spreadsheet-sdk")

const client = connectSpreadsheetServer({ baseUrl: "http://127.0.0.1:8079" })
const response = await client.canonical.execute("read_cells", {
  resource_id: "wb:wb-1",
  sheet_name: "Sheet1",
  selection: { kind: "range", ranges: ["A1:B2"] }
})
console.log(response.schema_version, response.operation)
```

In TypeScript, `execute<K extends OperationName>(operation: K, input: InputOf<K>)` returns
`Promise<OutputOf<K>>`. A wrong operation name or a wrong input shape is a compile error.

## Types

`OperationName`, `InputOf<K>`, `OutputOf<K>`, and `CanonicalErrorEnvelope` are generated
from `src/generated/canonical-registry.json`, which is itself generated from the Rust
registry. The generated TypeScript is checked in and drift-tested.

```bash
ASP_BINARY=../../target/debug/asp npm run generate:registry
npm run generate:types
npm test
```

## Rendering

`renderSheet` returns PNG bytes plus what the renderer reported. Image bytes cross the
adapter boundary rather than a canonical operation: the server runtime fetches
`GET /v1/artifacts/{handle}`, and the local runtime uses the WASM byte binding.

```js
const { connectSpreadsheetServer } = require("agent-spreadsheet-sdk")

const client = connectSpreadsheetServer({ baseUrl: "http://127.0.0.1:8079" })
const rendered = await client.workbook("wb:wb-1").renderSheet({
  sheet_name: "Sheet1",
  range: "A1:H40"
})
console.log(rendered.png.byteLength, rendered.fidelity, rendered.warnings.length)
```

`fidelity` is `"unknown"` and `warnings` is empty against a server that predates fidelity
reporting. A local runtime whose bindings have no artifact binding throws
`CapabilityError` rather than pretending.

The local runtime renders in process, with no LibreOffice and no host: the raster
renderer is compiled into the WASM module. `renderSheet` reads the bytes through
`readArtifact` and releases the session's artifact slot before returning, so a render
loop cannot evict its own earlier images.

```js
const fs = require("node:fs")
const { createWasmRuntime } = require("agent-spreadsheet-wasm")
const { createLocalSpreadsheet } = require("agent-spreadsheet-sdk")

const local = createLocalSpreadsheet({ runtime: createWasmRuntime({}) })
const workbook = await local.open(fs.readFileSync("book.xlsx"))

const rendered = await workbook.renderSheet({
  sheet_name: "Sheet1",
  range: "A1:F40",
  // `fast` trades bytes for latency, `best` the other way. Geometry never changes.
  png_level: "fast"
})
console.log(rendered.renderer, rendered.width, rendered.height, rendered.png_level)
fs.writeFileSync("sheet.png", rendered.png)

await workbook.dispose()
```

### Worker mode

Rendering and recalculation are synchronous CPU work, so in a browser they belong off
the UI thread. `worker` runs the bindings behind a Web Worker (or `worker_threads` in
Node) with the same surface on the main thread.

A live bindings object cannot cross a worker boundary, so worker mode takes the module
the worker should import — or an explicit port you already own:

```js
const { createLocalSpreadsheet } = require("agent-spreadsheet-sdk")

const local = createLocalSpreadsheet({
  runtime: { module: "agent-spreadsheet-wasm" },
  worker: true
})
console.log(typeof local.close, (await local.capabilities()).includes("screenshot_sheet"))
await local.close()
```

Worker mode is on by default in browsers when `Worker` exists and the runtime is a spec
the SDK can move, and off in Node unless you ask for it. `local.close()` shuts the
worker down; `worker: false` keeps everything on the calling thread.

## Errors

One hierarchy, rooted at `SpreadsheetError`:

| Class | Thrown when |
| --- | --- |
| `CanonicalOperationError` | An adapter returned a canonical error envelope. Carries `code`, `operation`, `path`, `details`, and the raw `envelope`. |
| `CapabilityError` | The live runtime does not advertise the operation. Thrown before any transport. |
| `TransportError` | A non-canonical failure: an unreachable host, a proxy error page, a body that is not an envelope. Carries `status` and `body`. |

WASM rejections are decoded into `CanonicalOperationError`, never rethrown as raw strings.
HTTP failures use the route's pinned status table (`CANONICAL_ERROR_STATUS`).

```js
const { CanonicalOperationError, CapabilityError, connectSpreadsheetServer } =
  require("agent-spreadsheet-sdk")

const client = connectSpreadsheetServer({ baseUrl: "http://127.0.0.1:8079" })
const fork = await client.workbook("wb:wb-1").createFork()

try {
  await fork.write({ expected_revision: "stale", mode: "apply", ops: [] })
} catch (error) {
  if (error instanceof CanonicalOperationError) {
    console.log(error.code, error.canonicalStatus, error.path)
  } else if (error instanceof CapabilityError) {
    console.log("unsupported:", error.operation)
  } else {
    throw error
  }
}
```

## Capabilities

`client.capabilities()` is the authoritative live operation list: `GET /v1/operations` for
the server runtime, the binding's `operations()` for the local one. Operations outside it
throw `CapabilityError` before any bytes move.

```js
const { connectSpreadsheetServer } = require("agent-spreadsheet-sdk")

const client = connectSpreadsheetServer({ baseUrl: "http://127.0.0.1:8079" })
const capabilities = await client.capabilities()
console.log(capabilities.includes("create_fork"), capabilities.includes("inspect_vba"))
```

## just-bash

`agent-spreadsheet-sdk/just-bash` registers one `asp` custom command over the same SDK/WASM Rust runtime used by direct JavaScript callers. Install `just-bash` explicitly; it is an optional peer dependency. The shim handles arguments, JSON and VFS bytes—not spreadsheet algorithms.

### Retained sessions

- `asp session open VFS_PATH`: open once; return the resource ID, revision, actual evaluator counters and durability.
- `asp session op SESSION_ID OPERATION [--json JSON] [--request-id ID] [--baseline SESSION_ID]`: dispatch a canonical operation. Without `--json`, read JSON from stdin. `resource_id` is injected; mutating inputs still require `expected_revision`.
- `asp session info SESSION_ID`: inspect authoritative owner metadata.
- `asp session operations`: discover operations supported by the resident runtime. Use `asp schema OPERATION` for the canonical input shape.
- `asp session export SESSION_ID --output NEW_VFS_PATH`: save an XLSX snapshot without closing the session. Existing destinations are not overwritten.
- `asp session artifact SESSION_ID HANDLE --output NEW_VFS_PATH`: save an artifact.
- `asp session close SESSION_ID` or `asp session close --all`: release sessions in this VFS scope.

Writes, recalculation, reads, `session_history`, `checkpoint` and `staged_change` reuse one Rust owner. Ordinary edit/recalculate/read loops do not export/reopen XLSX or repeatedly ingest the evaluator. Supply a stable `--request-id` when an exact retry must reconcile with its original result; query it through `session_history` with `action: "outcome"`. Undo/redo, branches, checkpoints and staged approvals use the same canonical operations as native sessions.

**Durability is `memory`.** History and approval state last only for the runtime/session lifetime. An XLSX export saves the document and caches, not the journal. Closing a session or terminating its worker loses its memory history. Storage durability and rename behavior depend on the host VFS; the adapter does not claim fsync, crash persistence, or protection against writers outside the adapter. Sessions cannot be addressed through a different VFS scope, even if it shares the same command registration.

```js
const { readFile } = require("node:fs/promises")
const { Bash } = require("just-bash")
const { createWasmRuntime } = require("agent-spreadsheet-wasm")
const { createAspCommand } = require("agent-spreadsheet-sdk/just-bash")

// The host seeds the sandbox. The command itself accesses only its VFS.
const asp = createAspCommand({ bindings: await createWasmRuntime() })
const bash = new Bash({
  files: { "/workbook.xlsx": await readFile("book.xlsx") },
  customCommands: [asp]
})
try {
  const opened = await bash.exec("asp session open /workbook.xlsx")
  if (opened.exitCode) throw new Error(opened.stderr)
  const { resource_id, revision_id } = JSON.parse(opened.stdout)
  const edited = await bash.exec(`asp session op ${resource_id} write --request-id edit-1`, {
    stdin: JSON.stringify({ expected_revision: revision_id, mode: "apply", ops: [
      { kind: "set_cells", sheet_name: "Sheet1", cells: { A1: { kind: "value", value: 42 } } }
    ] })
  })
  if (edited.exitCode) throw new Error(edited.stderr || edited.stdout)
  const saved = await bash.exec(`asp session export ${resource_id} --output /updated.xlsx`)
  if (saved.exitCode) throw new Error(saved.stderr)
  const output = await bash.fs.readFileBuffer("/updated.xlsx")
} finally {
  // Stop new calls, drain accepted commands, and release resident owner slots.
  await asp.dispose()
}
```

### One-shot file commands

`asp op OPERATION --bind VFS_PATH [--baseline VFS_PATH] [--json JSON] [--output VFS_PATH|--in-place]` opens a temporary owner, executes once, exports when required, and disposes it. This is convenient for individual file operations, not warm loops. `asp operations` lists this one-shot subset; `asp schema OPERATION` and `asp example OPERATION` expose registry-derived inputs.

File-command revisions are SHA-256 byte generations, not temporary resident tokens. In-place publication rechecks the captured source under the adapter's VFS write lock. Writes use a temporary file and `mv`; save-as refuses existing destinations. This coordinates adapter writers, not arbitrary external VFS writers. Staging requires a retained session and is rejected by one-shot commands.

just-bash 3.4.2 needs Node.js 20.18.1 or newer, and its `js-exec` bridge needs an ESM host. The CommonJS example above requires Node.js 22.12+ to `require` the ESM WASM package; ESM callers can import `createWasmRuntime` directly.
Defaults match the WASM ceilings (64 MiB per workbook, 1 MiB per parameter document);
override them with `maxWorkbookBytes` and `maxParamsBytes`.

## Deprecated: `agent-spreadsheet-sdk/compat`

The 0.14 surface — `McpBackend`, `WasmBackend`, the legacy camel-case method layer, and
`stateless-byte-adapter` — moved to `agent-spreadsheet-sdk/compat` for one release. Every
export there is `@deprecated`. These compatibility exports remain present, but legacy parity is outside the 0.16 resident-runtime acceptance scope; use the canonical SDK interfaces above for new integrations.

Migration: replace `new WasmBackend({ bindings })` with
`createLocalSpreadsheet({ runtime })` plus `local.open(bytes)`. Replace
`new McpBackend({ transport })` with `connectSpreadsheetServer({ baseUrl })`, or with a
real MCP client if you want the MCP transport — the SDK no longer pretends to be one.
Legacy methods that flattened envelopes to `data` have no replacement: canonical envelopes
are returned whole, and `client.canonical.execute` is the typed escape hatch.

## Development

```bash
npm install
npm run build            # dist/cjs, dist/esm, dist/types
npm run typecheck        # includes the type tests under test-types/
npm test                 # build + typecheck + node --test

# integration harnesses
node scripts/run-generated-wasm-integration.js          # builds/reuses the WASM package
SPREADSHEET_MCP_BINARY=../../target/debug/agent-spreadsheet-mcp npm run test:server
```

The Rust operation registry is the source of truth for the taxonomy, schemas, and adapter
support. This package never hand-normalizes canonical semantics; see
`docs/architecture/surface-boundary-rules.md` rule 5.
