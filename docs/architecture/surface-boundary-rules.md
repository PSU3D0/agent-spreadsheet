# Surface Boundary Rules (Non-Negotiable)

Status: active guardrails; amended by canonical-operation Gate 1
Owner: ticket 48 (`tickets/48-canonical-operation-convergence`)

This document defines the hard boundaries across the Rust operation core, CLI, MCP, WASM, SDK, and external adapters. If a change conflicts with these rules, redesign it or amend this document and the capability matrix explicitly.

## Rule set

### 1) Shared semantics use opaque resources

- Canonical operations accept one opaque `resource_id`, never a filesystem path, raw workbook bytes, VFS path, workspace root, or transport envelope.
- Workbook, fork, and session resources share a typed-prefix identity namespace.
- State reads return `revision_id`; mutations require `expected_revision` and return `revision_before` / `revision_after`.
- Shared semantic behavior lives behind `execute_operation`; adapters bind resources and project responses.

### 2) File CLI remains stateless; explicit sessions may be resident

- Human file CLI commands operate on explicit file/output paths and must not require long-lived server sessions or MCP envelopes.
- Explicit resident session bindings are an additive mode governed by `resident-session-runtime.md`: they may auto-start a private local host and reuse an evaluator across CLI invocations. They do not change ordinary path-bound command behavior or put spreadsheet semantics in a transport.
- CLI adapter flags (`--dry-run`, `--in-place`, `--output`, `--force`) control bind/export/file-replacement behavior, not spreadsheet semantics.
- Canonical machine mode (`asp op`) binds a path to an ephemeral resource; mutation binds an ephemeral fork and exports/replaces atomically.
- CLI helpers that understand spreadsheet structure must compile to canonical operations. If they cannot, the canonical write/read model is incomplete. Only generic file copying, shell formatting, and path UX may remain CLI-only.

Primary adapter boundary:
- `crates/agent-spreadsheet/src/cli/**`
- `crates/agent-spreadsheet/src/runtime/stateless.rs`

### 3) Hosts project workspace and persistence; session semantics are shared

- MCP maps workspace discovery, workbook cache, forks, checkpoints, staged approvals, and artifacts to canonical resource ids.
- Resident document/evaluator/transaction/history semantics live in the shared Rust runtime, not exclusively in MCP. Native and portable hosts provide scoped lifecycle/storage capabilities; memory-only storage never claims native crash durability.
- MCP transport envelopes, annotations, timeouts, and tool registration are adapter concerns.
- MCP wrappers must call the canonical dispatcher and must not normalize or reimplement spreadsheet behavior.
- Tool annotations use the operation's worst-case risk; request-aware hosts may use `risk_for(request)`.

Primary adapter boundary:
- `crates/agent-spreadsheet-mcp/src/server.rs`

### 4) WASM is byte/session oriented

- WASM creates a resource from workbook bytes and exports bytes from a resource.
- WASM must not require workspace roots, repository scanning, host paths, or MCP fork identity.
- One generic operation-dispatch binding is preferred; per-operation bindings may exist only when generated from the canonical registry.
- Memory/byte ceilings are enforced before workbook allocation.

Primary adapter boundary:
- `crates/agent-spreadsheet-wasm/**`

### 5) SDK is a programmatic object model over canonical runtimes, not a semantics fork

- Every SDK runtime exposes `canonical.execute(operation, input)` with canonical request/response JSON; workbook, fork, and view objects are generated or thin wrappers over it that only inject resource identity and tracked revisions.
- The SDK has exactly two runtimes: local (WASM byte sessions) and server (the canonical `/v1` HTTP route on the MCP server process). It is not an MCP client; MCP hosts use an MCP client.
- SDK must not hand-normalize canonical response semantics, fabricate unsupported success, or advertise operations not listed by its live runtime.
- Runtime-specific concerns are resource creation/export, transport, error status projection, image bytes for screenshots, and explicit capabilities.

Primary adapter boundary:
- `npm/agent-spreadsheet-sdk/**`

### 6) External adapters are projections only

- just-bash registers one `asp` custom command supporting `asp op <operation> --json <payload>`. Explicit resident binding/lifecycle may extend its adapter controls while ephemeral path binding remains supported; lifecycle is not a separate spreadsheet operation taxonomy.
- It uses the SDK dispatcher and just-bash `ctx.fs`; it carries no operation taxonomy, operation-specific parser, or spreadsheet logic.
- Other adapters follow the same rule: bind resource, dispatch canonical operation, serialize canonical response.

### 7) Proof and value freshness are explicit

- Cache presence never authorizes a clean/proved result.
- Recalculation and verification report evaluation coverage and same-revision freshness.
- Value-bearing reads report known calculation state without implying evaluation.
- Derived metrics report coverage/sampling and never flatten into exact metadata.

### 8) Mutation safety is semantic, not adapter-specific

- Canonical writes support pure preview, apply, and explicit stage. Resident hosts label storage scope; durable stage is advertised only with backed persistence. Stage/checkpoint catalog generations are separate from workbook state revisions, so a newly created approval does not invalidate itself.
- Atomic execution defaults on; non-atomic partial effects are structured results, not transport-only errors. Atomicity is an externally unchanged state/revision on failure, not a requirement to copy the whole workbook: validated bounded deltas/inverses are permitted for resident hot paths under the shared transaction/recovery contract.
- Every mutation uses revision CAS.
- Every write op kind passes through one dispatcher for preview/apply/stage.

## Additive registry status

The additive dispatcher registers the complete discovery/read/search/analysis set plus canonical `write` in `crates/agent-spreadsheet/src/operations.rs`. Existing CLI/MCP names remain registered unchanged. Compatibility wrappers project dispatcher data only where reconstruction is lossless; incompatible legacy response families remain separate and documented rather than claiming parity. `asp op` owns path binding and never forwards adapter paths into canonical requests. Read bindings use an ephemeral workbook resource; mutable bindings use an ephemeral fork and require explicit atomic `--output` or `--in-place` export (except pure write preview); two-resource verification binds `--baseline` and `--bind` into one ephemeral repository. Durable fork/history/checkpoint/stage operations are not advertised by the stateless CLI. `asp registry --all` remains the host-independent 31-descriptor generator projection, while `asp operations` is adapter- and runtime-filtered. `list_workbooks` accepts no `resource_id` and returns discovered typed resource ids inside `data.workbooks`.

No default MCP router change is authorized by this tranche. Every registered operation has a closed request schema, versioned output schema, policy metadata, and structured errors. Canonical write provides pure preview, one-bundle stage, revision CAS, atomic rollback by default, and structured non-atomic partial results. The live MCP projection suite locks legacy data-only responses and errors while the router remains in compatibility mode.

## Enforcement hooks

- Canonical design: `docs/architecture/canonical-operation-surface.md`
- Capability inventory and migration status: `docs/architecture/surface-capability-matrix.md`
- CLI/MCP catalog drift check (including `asp operations` and `asp op`): `scripts/check_surface_matrix_drift.py`
- Local enforcement:
  - `python3 scripts/check_surface_matrix_drift.py`
  - `cargo test -p agent-spreadsheet surface_matrix_drift_check`
- Cross-surface canonical parity fixtures are required per operation and response branch.

## Change control

Any boundary exception must include:

1. The rule being changed and why.
2. Canonical design and capability-matrix updates.
3. Drift/parity test updates.
4. Evidence that behavior was not duplicated in an adapter.
