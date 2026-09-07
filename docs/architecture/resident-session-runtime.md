# Shared resident session runtime

Status: implementation contract (not a statement of shipped behavior).

This work extends the canonical-operation surface. It does not replace that registry, turn file commands into implicit sessions, or change FormulaPlane defaults. For explicit resident bindings it narrowly supersedes the former stateless-only CLI, MCP-exclusive persistence ownership and mandatory whole-state-copy implementation wording. The corresponding active boundary rules and capability planning notes are amended with this contract; actual discovery is enabled only when implemented.

## Goals and host decisions

- One portable Rust session/mutation/calculation implementation, used by native CLI/MCP and WASM/SDK/just-bash adapters.
- Native explicit session commands auto-start/connect to a private local resident host. Ordinary path-bound file commands retain their current atomic writeback/export behavior.
- Native sessions journal acknowledged edits by default. Portable sessions explicitly report memory-only or host-backed persistence; just-bash uses only its VFS. No browser or virtual shell requires an OS daemon.
- Preserve supported document formatting, formulas, names, layout and workbook features through export. Existing Umya fidelity is the baseline, not a claim of universal lossless OOXML preservation.
- Delivery is reviewed, CI-validated merge, with normal main-branch container CI allowed. No release tags or versioned package publication are part of this work.

## Ownership and boundaries

The portable session owner contains the authoritative document, a disposable derived Formualizer evaluator, logical revisions/calculation coverage, transaction/history state, and bounded artifact ownership. Host paths, VFS paths, workspace scanning, authentication, process startup and durable I/O remain adapter concerns.

Use the existing canonical registry and dispatcher. Extract WASM's handwritten write/recalc/verify orchestration into shared Rust instead of adding equivalent native and JavaScript implementations. Add a resident backend to the existing canonical write executor. All operation families remain supported through their existing semantic implementation; unsupported incremental synchronization takes an explicit safe evaluator-rebuild route.

The evaluator must not become a second source of truth. Successful simple value/formula edits synchronize it without workbook export/reimport. Complex structural/name changes may invalidate it for one rebuild. Style-only changes should preserve applicable calculation proof where the effect classification proves no calculation change. A failed synchronization after a committed document mutation invalidates the evaluator; it cannot retroactively report the document mutation as rolled back.

Simple cell/matrix edit loops are the required hot path. Avoid whole-XLSX serialization and whole-document copying merely to make each small edit atomic. Broader operations may use conservative candidate-state fallback, with honest diagnostics and separate measurements. No unsafe Send/Sync implementation may be used to move an evaluator between hosts/threads.

## Observable state

Maintain these distinct concepts:

- Document/history identity: the committed edit/head state, independent of XLSX ZIP byte layout.
- Public workbook state revision: opaque CAS/cursor token which changes for observable document/value/calculation transitions and cannot ABA across undo, checkout, restore or restart. Stage/checkpoint catalog-only changes have a separate catalog generation and do not change workbook state.
- Calculation stamp: which document/configuration/runtime evaluation context is covered, including the existing clean/errors_found/partial/not_evaluated states.
- Export stamp/content hash: the snapshot successfully written to a destination; exporting does not itself edit the logical document or trigger recalculation.

Preserve existing conservative state-revision behavior when complete calculation publishes new values/coverage. Calculation is not a history edit, but can change the public state/cursor revision; an approval tied to an older observable state must not silently apply. Add explicit document/calculation/export metadata rather than redefining old revision strings as serialization hashes. Update closed schemas, generated SDK types and parity fixtures together.

Reads remain explicit about stale/unknown values and do not implicitly calculate. A session write dirties relevant coverage. A complete incremental calculation may combine prior trusted unchanged coverage with all invalidated work successfully recalculated at the same revision. Number of results changed, vertices evaluated and total formula count are different metrics.

Partial/failed/cancelled calculation cannot publish partial caches as current. Retained partial engine state must be safely resumed or invalidated/rebuilt before later success. Verification must not mutate either bound session or manufacture proof from cached values.

## Transactions, journals and history

All changes to one session have a single serialized commit owner. Concurrent sessions may run independently. Reads/export snapshot capture see a coherent committed revision, never a half-applied document/engine/journal transition.

Canonical preview is pure. Apply/stage/CAS/atomic/non-atomic behavior stays with the existing shared write executor. Stage checks the current workbook state revision and binds its approval to that exact revision; creating/discarding a bundle changes the separate catalog generation, not the workbook revision. Immediate staged apply therefore succeeds, and creating another alternative bundle does not self-invalidate the first. Apply requires both caller CAS and the bundle's approval revision to match current workbook state. A write, published calculation, undo/redo, checkout or restart-invalidated calculation state makes older approvals stale. Staged catalogs carry their own generation for list/cursor consistency. Staged apply uses the same executor and rejects stale approvals. Atomic failure leaves visible state and revision unchanged; non-atomic failure records precisely the successful effects and reports structured partial results.

For journaled sessions:

1. Validate/prepare against the checked revision without publishing effects.
2. Commit a versioned, integrity-checked record through the host persistence boundary.
3. Publish the committed state, then acknowledge the actual durable outcome.

An equivalent ordering is acceptable only with explicit rollback/recovery proof. Persistence can commit and then report an error: use persisted session-scoped reconciliation identities bound to request fingerprints, reject identity reuse with different inputs, and document outcome retention/expiry. Publication after commit must be infallible for ordinary errors or poison/recover the session before serving more requests. Journal failure must not be reported as zero effects after visible state was published. A crash after durable commit but before response is an uncertain client outcome, not evidence that the write failed; support explicit request identity/outcome reconciliation without changing normal stale-CAS requests into apparent success.

Every acknowledged persistent transition, including HEAD/branch selection, undo/redo/checkout and approval/checkpoint catalog changes, is represented in the authoritative commit stream. A commit identifies its parent, resulting head/branch/catalog state, schema version, and deterministic prepared effects or sufficient deterministic replay inputs; non-atomic commits encode exactly the successful effects. Unknown required record kinds/versions fail recovery explicitly rather than being skipped. A complete journal commit is the recovery authority. HEAD/branch indexes and snapshots are recoverable projections, not separate competing commit points. Recover incomplete tails according to a documented format; do not silently ignore interior corruption. Never use engine memory or engine-local undo as durable history.

Native interprocess ownership uses real exclusive locking, not exists-check/write or age-based lock stealing. Undo/redo/checkout/branch/stage paths use the same ownership discipline. Replay follows parent ancestry, not append-order prefixes. A branch cannot accidentally replay another branch's operations. Redo follows its selected branch path. Existing session formats require compatibility/migration tests; untrusted old snapshots cannot authorize an incorrect ancestry reconstruction.

Reconstruction after restart establishes committed edits, not a false promise to reproduce arbitrary volatile/random/time/external-data results. Either persist sufficient evaluation context/results or mark calculation untrusted until explicitly recalculated. Preserve supported settings and disclose the chosen recovery policy.

Portable storage is host-owned and asynchronous where necessary. Expose a minimal load/commit/checkpoint interface with CAS and stated guarantees; do not add a storage platform. Memory VFS persistence lasts only as long as that VFS host. Do not claim fsync/crash durability from an ordinary VFS move. Durable staging/checkpoints are advertised only when actually backed, or explicitly labeled with their supported persistence scope.

## Core persistence implementation boundary (M2, not adapter adoption)

The journaled calculation path prepares formula cache updates on the retained evaluator, commits only calculation coverage/revision metadata, then publishes those updates into Umya. It does not serialize XLSX or construct/re-ingest an evaluator on the normal dirty cell-edit/calculation loop. Recovery deliberately distrusts derived caches; concrete calculation-dependent edits remain materialized in prepared transactions. Evaluation failure before publication poisons the owner until recovery rather than serving an old Current stamp. Unexpected postcommit publication failure also poisons.

Checkpoints are catalog entries identified by their creation request ID, retaining head, workbook revision and optional label. Create/delete leave workbook CAS and staged approvals unchanged; restore is a separate document/history operation. Accepted true no-op requests have durable receipts without changing document ancestry, workbook CAS or catalog generation. Preview remains untracked. No receipt expiry is implemented: outcomes are retained for the lifetime of the journal.

Legacy SessionStore and old CLI commands are still legacy mutators, not the resident authority. Import requires a private frozen copy made while all legacy writers (including older binaries and existing handles) are excluded. The copy must contain `resident-import-frozen.json` with `{"frozen":true,"base_sha256":"<hash of copied base.xlsx>"}`. This is an explicit host assertion and immutable-base binding, not a lock-stealing or automatic live-cutover protocol. New-version ordinary SessionStore opens and mutations refuse frozen copies. Import does not rewrite the copied event/projection stream. M3 must durably preserve base plus imported journal and fence old entrypoints before selecting the new authority; the frozen marker alone cannot fence older binaries.

Native journal roots must be private, trusted and nonreplaceable with trusted ancestors. Creation is single-level and requires an already durable parent; recursive ancestor creation is intentionally unsupported. Unix journal opens use `O_NOFOLLOW` plus opened-file regular/private checks. File and directory synchronization failures remain uncertain until successful reconciliation barriers; persistent failures cannot reconcile as Committed. Non-Unix ACL privacy and directory barriers are not implemented, and neither Windows nor macOS guarantees have been execution-tested here. M3 must enforce suitable host guarantees or refuse unsupported durable modes. OS locking on the old append path removes age-based lock stealing, but does not make the legacy event/projection layout transactional.

## Native CLI and MCP

- Explicit resident binding/session lifecycle is additive. Existing stateless file bindings do not auto-connect to a daemon.
- The native host embeds the same shared runtime/dispatcher used by MCP. Do not put spreadsheet semantics into a second service or require CLI consumers to speak MCP envelopes.
- Use a small, authenticated private local transport implemented with existing/standard protocol libraries. A loopback address alone is not authentication. Scope startup/discovery/ownership to the intended workspace/user, bind loopback only, protect credentials/state files, validate paths and request sizes, and handle startup races and stale host discovery safely.
- Support the repository's native platforms; no silent Unix-only assumption. Validate real cross-process reuse, host shutdown/restart, and failure behavior.
- API/MCP outer timeout or disconnect must not report a definitive effect-free failure for a blocking mutation that continues and commits. Finish/reconcile accepted mutations and keep calculation cancellation distinct.
- External source changes do not silently overwrite resident work. Export captures a named immutable workbook/calculation revision; its success updates only that snapshot's export stamp, not a concurrently edited revision. Check the bound source generation at replacement time, with explicit force/save-as choices. State the residual check/rename race against non-cooperating external writers: ordinary filesystem replacement is not universal filesystem CAS. No full file read/hash on every warm in-memory operation merely to emulate residency.

## WASM, SDK, workers and just-bash

- Cache compiled modules if useful, not a global authority over every client's resources. Runtime/session IDs are resolved inside the owning runtime/host scope. Distinct runtimes and virtual hosts cannot access each other's workbooks or artifacts even if paths or IDs are supplied explicitly.
- SDK local workbook objects keep their existing useful lifecycle shape. Both direct and worker-backed calls use the shared Rust runtime. Distinguish closing a client, detaching a durable session, disposing a memory-only session, and terminating an owned worker.
- Unexpected worker exit rejects pending operations with honest outcome semantics; closed clients cannot reopen or dispatch through a cached dead runtime. Restart/recovery must not silently reuse invalid handles.
- Extend just-bash with explicit resident session binding/lifecycle while preserving its ephemeral `asp op --bind` behavior. Repeated exec calls in one virtual host can select one live session; separate hosts containing the same path remain isolated. All filesystem access stays through `ctx.fs`.
- just-bash remains a thin registry/SDK projection. Do not add operation-specific spreadsheet logic or an independent schema/operation list. Update adapter capability plans/discovery rather than bypassing them.
- Check import/parameter limits before allocation, and bound session/artifact/history ownership. Never silently evict the only copy of unexported memory-only edits. Native/portable admission limits are not a hard RSS guarantee.

## Implementation milestones and seams

1. **Dependency and portable owner**: move the coherent engine dependency set to 0.9, remove the old workbook vendor override after checking its clock fix is upstream; extract reusable evaluator and shared resident state/coverage. Preserve one-shot behavior. Add core retention and export-fidelity tests.
2. **Mutation and durable history**: resident canonical write backend, safe incremental common edits/rebuild fallback, stage/CAS/rollback, journal commit/recovery, ancestry/locking/ABA fixes, host persistence contract. Add deterministic failure-injection and restart tests before advertising durable success.
3. **Adapters and lifecycle**: shared native MCP/CLI routing, auto-start private native host, portable WASM ownership, SDK direct/worker lifecycle, host-scoped resident just-bash and VFS persistence/export. Update canonical docs, boundary rules, matrix and generated surfaces in lockstep.
4. **Integration, measurements and review**: actual MCP and cross-process CLI tests; generated real WASM package, SDK direct and Node/browser worker tests; actual just-bash Bash/VFS exec tests; benchmark cold/import/warm/export separately. Fresh independent functional and durability/security review, fixes, ordinary CI, then parent-owned merge.

One writer owns a worktree at a time. Advisory reviewers are read-only. Milestone completion is not end-to-end completion; the adapter and artifact gates below remain mandatory.

## Acceptance and performance evidence

Extend the existing test/scenario/wasm runners, not a new general validation framework. Tests must execute, not silently skip because generated bindings or a native binary are absent.

Required correctness cases:

- repeated dependent value/formula edits, dirty reads, complete/error/partial calculation and recovery;
- identical shared-fixture results and compatible envelopes across core, CLI, MCP, SDK/WASM and just-bash;
- pure preview, stage/apply, stale CAS, same-resource competing writes, indexed non-atomic outcomes, atomic failure;
- branch ancestry, undo/redo/checkout, no ABA, journal failures including commit-then-error, interrupted tails, crash after commit/before reply, restart after each lifecycle/catalog transition, exact mixed-success replay and unsupported record/version rejection;
- immediate stage/apply success, multiple alternative bundles without self-invalidation, and stale approval after write/calculation/history movement/restart;
- source conflicts, export failure/retry, exported values/formulas plus formatting/names/layout parity;
- worker death, owned-worker shutdown, runtime/resource/artifact isolation, two VFS hosts with the same path, repeated Bash execs and host-backed recovery;
- unauthenticated/private-host requests, forged/cross-workspace resource IDs, unintended browser-origin access, path/symlink escape, compressed/decompressed import limits, startup races and stale credentials;
- concurrent edit/export, late external replacement, post-commit evaluator failure without false rollback, and unchanged stateless file writeback/capability/discovery/schema drift gates.

Required reuse evidence on common edit loops: one initial evaluator construction/ingest, no XLSX export/reimport between edits, real invalidation/recalculation, correct changed results, and one explicit final export. Count rebuild fallbacks and do not mislabel them warm. Include a structural fallback control.

Benchmark actual adapters on the same generated source fixture and fixed edits. Report startup/compile/import, first evaluation, mutation, warm dirty recalculation/read, journal work and final export separately; retain stateless cold-file controls. Use native release builds and real packaged/generated WASM/SDK/just-bash, with source/toolchain provenance. Timing runs must be sequential and labeled for shared-host limits. No blanket sub-second or cross-platform speed claim from synthetic counters or mock transports. Larger owner-provided workbooks may be tested privately; no workbook content or raw financial output is committed or published.

Do not weaken existing timing/size budgets to get a green gate. If a legitimate new capability exceeds one, report the measured cause and obtain a deliberate decision. Keep FormulaPlane off by default throughout this work.
