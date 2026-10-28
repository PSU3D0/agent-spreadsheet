# Local development source-runtime proof

The default plugin still loads exactly registry agent-spreadsheet-wasm 0.16.0 and its existing lock integrity. That package lacks `recalculateSourceXlsx`; the source command fails with an actionable message on it. No resident-session fallback is used. A new binding has not reached the default package merely because this checkout builds successfully.

For a trusted local development build, use matching generated JS **and** WASM from the same source. Never replace the installed registry binary underneath its old glue. From the agent-spreadsheet source root:

```sh
node scripts/build-wasm-package.js --target nodejs --profile wasm-release --no-opt --out-dir target/packet2c-source-node
```

This creates an ignored local build, not a published package. The profile uses opt-level z, fat LTO, one codegen unit, panic abort and stripping; this invocation skips wasm-opt. Keep the source identity and WASM digest separately from registry version/SRI.

Create `target/packet2c-source-node/loader.mjs` beside that build:

```js
import { createRequire } from "node:module";
const require = createRequire(import.meta.url);
export async function createWasmRuntime() {
  return require("./agent_spreadsheet_wasm.js");
}
```

The optional `--dev-runtime LOCAL_LOADER.mjs` prefix explicitly executes that trusted local module's `createWasmRuntime()`. It accepts paths, not HTTP/file URLs; it does not download anything, mutate global configuration, change dependency pins or consult an environment variable for production runtime selection. Default doctor/transport continues to use registry 0.16.0. Development doctor and source receipts label the selected runtime as development with no registry-version claim.

From the skill folder, substituting the actual absolute loader path:

```sh
node scripts/run.mjs --dev-runtime /absolute/checkout/target/packet2c-source-node/loader.mjs doctor
node scripts/run.mjs --dev-runtime /absolute/checkout/target/packet2c-source-node/loader.mjs source-recalculate /path/input.xlsx --check
node scripts/run.mjs --dev-runtime /absolute/checkout/target/packet2c-source-node/loader.mjs source-recalculate /path/input.xlsx --options /path/options.json --output /path/repaired.xlsx
```

Without `--output`, no file is written. `--check` is explicitly read-only and cannot combine with `--output`; successful checking returns exit 0 even when `current:false`. This means calculation succeeded and the returned byte comparison found stale caches, not that a repaired file was published. A command without either flag also returns a receipt and discards its calculated bytes. Explicit output follows ordinary filesystem overwrite behavior, including the input itself if explicitly selected. Parent directories must exist. There is no shared-file CAS or atomic publication promise.

The receipt is JSON on stdout: runtime provenance, resolved input/output paths, `current` (exact input/result byte equality), input/result SHA256, result byte count, and unchanged binding metadata. A failure writes details to stderr and exits nonzero. Calculation and metadata decoding complete before any destination write; malformed/refused input or options leave an existing destination untouched. The transport frees every returned wasm-bindgen result in finally, including decoding/write failures.

Options are passed as raw JSON to the core binding. Example:

```json
{"errorLocationLimit":3,"externalLinks":"cached","limits":{"maxFormulaCells":200000,"maxColumns":1024}}
```

The binding owns all workbook, admission and calculation semantics. It accepts every documented upstream limit override plus sample/external-link settings, rejects unknown fields, and independently keeps this adapter's inherited 64 MiB input/output and 1 MiB JSON guards. The effective upstream output limit is capped before construction; raising `maxOutputBytes` does not bypass the adapter guard. These are adapter bounds, not universal Node/WASM capacity. JS custom EvalConfig, fixed clock/seed/timezone and cancellation tokens are unavailable and are rejected; native Rust retains the full upstream options API.

The direct binding receives original bytes before createSession/Umya materialization. It returns `Uint8Array` bytes separately from `metadataJson`; the transport never stringifies workbook bytes or recreates summaries from samples. Successful Excel errors retain exact error totals and sampled/truncated locations. Unsupported input is an error without result bytes.

For local proof against matching generated assets:

```sh
FORMUALIZER_TEST_RUNTIME=/absolute/checkout/target/packet2c-source-node/loader.mjs node --test test/*.test.mjs
```

That variable selects a loader only inside the test harness; spawned commands still use the explicit CLI flag. Four generated-source test cases are skipped when it is absent. The original nine registry read/export tests continue to run against the pinned installed package. Cached B1=42 in the Packet1 probe remains transport evidence only.

This is local Node source-runtime proof. It does not certify Code, Cowork, browser execution, artifact delivery or publication.
