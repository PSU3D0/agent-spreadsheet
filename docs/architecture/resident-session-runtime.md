# Resident spreadsheet runtime (0.16)

## One execution core

The Rust `SpreadsheetOperation` registry defines the 32 canonical operations, their schemas and adapter capabilities. Native CLI/MCP and WASM SDK calls share the dispatcher and resident transaction/history implementation. Adapters handle transport, resource binding and host I/O; they do not implement spreadsheet algorithms.

A resident owner keeps an authoritative Umya document and a reconstructible Formualizer evaluator. Ordinary edit/recalculate/read loops neither serialize/reparse XLSX nor re-ingest an unchanged evaluator. Structural changes may require evaluator reconstruction. Recalculation publishes formula caches into the original document rather than replacing its OOXML representation.

XLSX export is a cold projection, not the journal. A lazy snapshot cache is keyed by the complete resident revision so repeated export of an unchanged revision returns identical bytes. Formula definitions, cached value types, names, styles and layout remain document state.

## Transactions, history and outcomes

- Mutations use revision CAS and ordered canonical operations.
- Prepared effects and the original canonical response are committed in the same journal record. There is no independent response cache acting as an outcome authority.
- A stable request ID reconciles an exact retry. Reusing it for different input is rejected. Callers should supply an ID when they need later outcome reconciliation.
- Calculation records include coverage, actual evaluator counts and diagnostics. Formula errors are reported as completed-with-errors, not an unqualified success.
- Undo/redo, branch navigation, checkpoints and staged approval/application use shared history state. Revision and branch identity prevent accidental stale mutation.
- Poisoned or uncertain outcomes are reported explicitly; accepted work is not described as rolled back merely because its caller stopped waiting.
- History and outcomes are bounded. Exceeding storage limits rejects new admission rather than silently evicting acknowledged history.

## Native CLI and MCP

Explicit native sessions automatically discover/start a private local host. Separate CLI invocations and MCP requests can reach the same resident owner. Ordinary stateless file commands do not require a surviving host and continue to perform file writeback.

The host uses bounded serialized owner lanes for non-Send evaluators. No unsafe Send/Sync assertion is used. Transport authentication binds requests and responses with domain-separated HMAC, nonce and body checks; loopback location alone is not authentication. Credentials, immutable bases, configuration and journals live under a private workspace/user-bound root.

Native history is journal-backed. Recovery verifies base identity, effects, records and original outcomes. Export captures a named revision and records its plan/receipt; uncertain retries retain their capture and destination-generation constraints. Force is not permission for an old pending export to overwrite a newer destination indefinitely.

Filesystem replacement is not universal CAS against arbitrary external writers. Native roots require trusted ancestors, locking and explicit synchronization; the implementation must not silently downgrade those guarantees.

Native release targets are Linux and macOS. Windows implementation, validation and binary publication are outside the 0.16 scope. Legacy session migration and the experimental offline migration/cleanup commands are not shipped. There is no promise that old persisted session stores can be imported.

Formualizer is the release-critical calculation backend. Existing optional LibreOffice support remains, but extended LibreOffice scheduling/validation is not a 0.16 acceptance gate.

## WASM, SDK and workers

The WASM API owns the same resident runtime with a bounded memory journal. Its metadata reports `durability: "memory"`, the actual revision, evaluator counters and serialization count. This is not disk durability: closing a session or terminating its runtime loses its history, checkpoints, approvals and receipts. XLSX export saves a document snapshot, not a restartable session journal.

The SDK supports direct bindings and worker RPC. Request IDs and owner metadata cross that boundary unchanged. Node worker exit, channel close and message-deserialization failure reject pending calls with transport/unknown-completion semantics. Explicit worker shutdown uses the SDK termination hook; browsers do not expose a general event for someone else calling `Worker.terminate()`.

Workbook disposal shares one in-flight attempt. Failed disposal remains retryable. Host cleanup drains accepted commands, attempts every resident handle and retains failed handles for another cleanup attempt.

The packaged loader caches initialization per JavaScript module instance. Clients deliberately sharing its bindings share that runtime; use separate workers/module instances when independent runtimes are required. Session IDs are random, and separate Rust `SessionApi` instances do not share owners.

Rust retains schema validation. Generated schema JSON is a projection of native Rust schema builders and is parity-tested; it is not an independently maintained schema source. No JavaScript schema-validator experiment is included. The published WASM size guard remains enforced, with an explicitly justified 0.16 revision rather than removing validation or renderer capabilities to meet the older ceiling.

## Genuine just-bash integration

`createAspCommand({ bindings })` provides two paths over the same SDK:

- `asp session open/info/op/operations/export/artifact/close` retains a Rust owner across shell commands. Writes, calculation, reads, history, checkpoints and approvals execute through the canonical dispatcher. Export is explicit and save-as refuses existing destinations.
- `asp op ... --bind ...` is a one-shot file operation. It opens a temporary owner, dispatches once, exports when required and disposes it. It is not the efficient path for repeated work.

Resource handles are scoped by just-bash's stable filesystem identity, not the per-command defense wrapper. A session from another VFS cannot be addressed through this command registration. All command filesystem access goes through `ctx.fs`; no native host daemon or host filesystem escape is used.

One-shot revisions are SHA-256 byte generations. The adapter verifies the file CAS token before translating it to its temporary owner's revision, and projects only protocol revision metadata back to file generations. It does not rewrite workbook values or keep a sidecar outcome store. In-place publication rechecks the captured source under the adapter's VFS write lock. These locks coordinate adapter writers only; persistence and rename semantics remain properties of the host VFS, not a claimed fsync guarantee.

Use `await asp.dispose()` at host shutdown to stop new commands, drain accepted work and release resident owner slots. See the SDK README for executable examples and request-ID usage.

## Umya 3 import compatibility

All native and portable document imports use the same Formualizer compatibility reader for published Umya 3.1.0's border-colour parsing bug. It restores colours directly into the authoritative document at cold import; no XML repair state survives and no warm-loop serialization is added. Renderer fixtures use this same production import boundary; scene and pixel goldens remain unchanged.

The paired cold writer corrects Umya's colour-blind style deduplication by repairing emitted font/fill/border colour definitions and cell, row, column and conditional-format style references from the authoritative document. **Theme/indexed/RGB identity and tint are retained; theme colours are not flattened.** This performs one ordinary Umya serialization followed by ZIP/XML projection correction, without parsing a workbook or rebuilding an evaluator. The temporary correction tables are discarded after export. Unspecified row height is emitted as the existing renderer's 15-point default, avoiding upstream's conflicting fallback. This targeted workaround does not promise support for OOXML metadata that Umya already omits.

The import reader rejects invalid repair metadata and bounds input/expanded parts to 256 MiB, aggregate declared expansion to 1 GiB, archive entries to 65,536, each style table to 100,000 entries and XML depth to 128. File and stream imports use one bounded input snapshot.

## Validation

The generated-runtime integration harness exercises actual WASM, just-bash and Node workers, including positive numeric oracles, warm-loop counters, exact retries, VFS isolation, history/approvals/checkpoints, native workbook/pixel goldens, exports and cleanup beyond the owner cap. Separate fault tests cover worker death, concurrent/failed cleanup, capability-discovery recovery and coordinated VFS publication.

Native tests exercise actual CLI/MCP processes, ownership, request reconciliation, history and export/recovery boundaries. Process-kill/injected-I/O tests do not constitute physical power-loss proof. Linux execution does not establish macOS acceptance; the release also requires ordinary macOS CI.
