# Canonical HTTP Route (`/v1`)

Status: shipped by ticket 5001 (tranche 50)
Owner: MCP / server adapter

The HTTP transport serves two surfaces from one process and one canonical dispatcher: the rmcp streamable-HTTP MCP service at `/mcp`, and a plain canonical HTTP route under `/v1`. The `/v1` route exists so the SDK server runtime (and any other programmatic client) can drive canonical operations without speaking MCP, while sharing the server's workspace, workbook cache, forks, checkpoints, and staged approvals.

The route is a transport adapter and nothing else. It binds a JSON body to a canonical operation, applies the same enabled-tool, timeout, and response-size policy as MCP tool calls, calls the shared dispatcher, and projects canonical envelopes onto HTTP status codes. It carries no spreadsheet semantics, no operation-specific parsing, and no path handling. See `docs/architecture/surface-boundary-rules.md` rule 3.

Implementation: `crates/agent-spreadsheet-mcp/src/http_route.rs`, mounted by `agent_spreadsheet_mcp::http_app`.

## Routes

### `POST /v1/op/{operation}`

The request body is the canonical input object for `{operation}` — exactly the object an MCP client would pass as tool arguments. An empty body is treated as `{}`. A body that is not a JSON object is `INVALID_REQUEST`.

Canonical `create_fork`/`list_forks` and explicit `fork:`/`session:` resources use the private native resident host; ordinary `wb:` requests remain stateless. Request identity is optional at adapter ingress. Without it, the adapter generates a fresh UUID before submission and returns it in `X-Agent-Spreadsheet-Request-Id` (HTTP) or result `_meta["agent-spreadsheet/request-id"]` (MCP). Each missing-ID call is distinct. For reliable retries of durable mutations, supply `X-Agent-Spreadsheet-Request-Id` (1–256 bytes), or MCP `tools/call` parameters `_meta["agent-spreadsheet/request-id"]`, not arguments or the JSON-RPC number. Reuse the same identity only with identical input; use `session_history` for uncertain delivery. Connection loss before receiving a generated ID can leave the outcome unknown; retrying without an ID is not blind-retry safe.

A successful call returns HTTP 200 and the canonical response envelope (`schema_version`, `operation`, optional `resource_id`, optional `revision_id`, `data`) as JSON. The body is byte-identical to the `structuredContent` an MCP call would carry.

A failure returns the canonical error envelope (`schema_version`, `error.code`, `error.message`, optional `error.operation`, optional `error.path`) with the status from the table below. The JSON is identical to what MCP `isError` results carry in their content.

### `GET /v1/operations`

Returns `operations_discovery_for(OperationAdapter::Mcp, capabilities)` for the live process, where `capabilities` includes configured VBA and the implemented native resident-history capability. This is the runtime-filtered view: an operation that is absent here will answer `CAPABILITY_UNAVAILABLE`.

### `GET /v1/registry`

Returns `registry_projection()` — the host-independent descriptor generator projection plus the canonical error schema. It is not runtime-filtered.

### `GET /v1/artifacts/{handle}`

Serves workspace-owned artifacts with their media type and `Content-Length`: screenshot handles use `artifact:sha256:<hex>` and `image/png`; canonical XLSX exports use their existing `artifact-<hex>` IDs and the XLSX media type. PNGs are bounded to 16 MiB, native XLSX artifacts to 64 MiB. IDs never authorize arbitrary export paths.

`{handle}` must be a well-formed `artifact:sha256:<64 lowercase hex>` screenshot handle or `artifact-<64 lowercase hex>` XLSX ID. The route never accepts, exposes, or echoes a filesystem path. Resolution is: canonicalize `<workspace_root>/artifacts`, refuse it if it is a symlink or not a directory, join the corresponding `<hex>.png` or `<hex>.xlsx`, refuse a symlink or non-regular file, refuse anything that does not canonicalize back inside the artifacts directory, enforce the media-type artifact ceiling, then verify that the file content hashes to the handle before serving a single byte. A file whose content does not match its name is never served.

Because the handle is content-addressed against this workspace, an artifact produced by a different workspace simply does not exist here and answers 404.

## Status mapping

| `CanonicalErrorCode` | JSON `code` | HTTP status |
| --- | --- | --- |
| `InvalidRequest` | `INVALID_REQUEST` | 400 |
| `StaleCursor` | `STALE_CURSOR` | 400 |
| `CursorMismatch` | `CURSOR_MISMATCH` | 400 |
| `RowExceedsBudget` | `ROW_EXCEEDS_BUDGET` | 400 |
| `UnknownOperation` | `UNKNOWN_OPERATION` | 404 |
| `ResourceNotFound` | `RESOURCE_NOT_FOUND` | 404 |
| `RevisionConflict` | `REVISION_CONFLICT` | 409 |
| `OperationFailed` | `OPERATION_FAILED` | 500 |
| `CapabilityUnavailable` | `CAPABILITY_UNAVAILABLE` | 501 |
| `OutcomeUnknown` | `OUTCOME_UNKNOWN` | 503 |
| `RecoveryRequired` | `RECOVERY_REQUIRED` | 503 |

Resident unknown outcomes require request-identity reconciliation, not a new identity or an effect-free retry assumption. A recovery-required owner rejects normal workbook access until explicitly recovered. Resident response deadlines return `OUTCOME_UNKNOWN` and the reconciliation identity while independently owned work continues; they never cancel an accepted mutation.

The three cursor/budget codes are client-correctable request faults (a cursor the client must re-acquire, a budget the client must lower), so they map to 400 alongside `INVALID_REQUEST` rather than to 409 or 413.

Adapter policy failures reuse the same envelope: an operation excluded by `SPREADSHEET_MCP_ENABLED_TOOLS` answers `CAPABILITY_UNAVAILABLE` (501), a response over `SPREADSHEET_MCP_MAX_RESPONSE_BYTES` answers `OPERATION_FAILED` (500), and a stateless call over `SPREADSHEET_MCP_TOOL_TIMEOUT_MS` answers `OPERATION_FAILED` (500). A resident response deadline instead answers `OUTCOME_UNKNOWN` (503), naming the resource/request identity for reconciliation; MCP carries the same canonical error.

The artifact route maps a malformed handle to `INVALID_REQUEST` (400), an unknown or unverifiable handle to `RESOURCE_NOT_FOUND` (404), and an over-ceiling object to `OPERATION_FAILED` (500).

## Bind and auth policy

Bind policy is unchanged from the MCP HTTP transport: `SPREADSHEET_MCP_HTTP_BIND`, default `127.0.0.1:8079`, loopback only.

**The `/v1` route has no authentication, exactly like `/mcp`.** Anything that can reach the port can read and mutate every workbook under the workspace root within the configured tool policy. The default loopback bind is the security boundary. If you bind to a non-loopback address, put your own authenticating proxy in front of it; TLS, CORS, and auth are explicitly out of scope for the server process.

There is no workbook upload or arbitrary source path. Native `create_fork` creates durable isolated owners only from authorized workspace resources or resident parents. The server keeps working within its pinned workspace authority.

## MCP image content

`screenshot_sheet` results cross the adapter boundary as bytes, not as a canonical retrieval operation. On MCP, a successful canonical `screenshot_sheet` result appends a `Content::image(base64, media_type)` item after the text content; `structuredContent` is unchanged and still carries only the path-free artifact handle. The base64 payload is charged against the response-size limit, and an image that would exceed it fails the call rather than truncating. Over HTTP, the same bytes are fetched from `GET /v1/artifacts/{handle}`.
