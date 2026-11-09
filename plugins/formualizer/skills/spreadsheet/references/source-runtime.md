# Source-runtime recalculation

The bundled stateless engine (backed by Formualizer 0.14.0), which includes `recalculateSourceXlsx` out of the box.

```sh
node scripts/xlsx.mjs doctor
node scripts/xlsx.mjs source-recalculate /path/input.xlsx --check
node scripts/xlsx.mjs source-recalculate /path/input.xlsx --options /path/options.json --output /path/repaired.xlsx
```

For an optional local development build, pass `--dev-runtime /absolute/checkout/target/practical-source-node/loader.mjs` before the command.

Without `--output`, no file is written. `--check` is explicitly read-only and cannot combine with `--output`; successful checking returns exit 0 even when `current:false`. The receipt describes calculation coverage separately; byte inequality alone does not establish cache freshness. A command without either flag also returns a receipt and discards its calculated bytes. Explicit output follows ordinary filesystem overwrite behavior, including the input itself if explicitly selected. Parent directories must exist. There is no shared-file CAS or atomic publication promise.

The receipt is JSON on stdout: runtime provenance, resolved input/output paths, `bytesUnchanged` and `bytes_unchanged` (exact input/result byte equality), plus legacy `current` with the same byte-only meaning, input/result SHA256, result byte count, and unchanged binding metadata. A failure writes details to stderr and exits nonzero. Calculation and metadata decoding complete before any destination write; malformed/refused input or options leave an existing destination untouched. The transport frees every returned wasm-bindgen result in finally, including decoding/write failures.

Options are passed as raw JSON to the core binding. Example:

```json
{"profile":"workbook","requireComplete":false,"calculation":{"iteration":{"mode":"workbook"},"workbookSeed":"18446744073709551615","fixedNowUtc":"2025-01-02T03:04:05Z","timezone":"utc"},"errorLocationLimit":3,"externalLinks":"cached","limits":{"maxFormulaCells":200000}}
```

The binding owns all workbook, admission and calculation semantics. It accepts every documented upstream limit override plus sample/external-link settings, rejects unknown fields, and independently keeps this adapter's inherited 64 MiB input/output and 1 MiB JSON guards. The effective upstream output limit is capped before construction; raising `maxOutputBytes` does not bypass the adapter guard. These are adapter bounds, not universal Node/WASM capacity. File-facing requests default to `profile:"workbook"`; `profile:"strict"` explicitly retains historical source guards. Native Rust `XlsxRecalculateOptions::default()` remains strict. Workbook defaults admit the sparse Excel column range (16,384) while retaining byte/cell/formula bounds. `requireComplete` is independent of the profile.

`calculation` uses the shared upstream strict JSON schema: `iteration:{mode:"workbook"|"disabled"|"iterate",maxIterations?,maxChange?}`, `cycleDetection:"runtime"|"static"`, `iterationInitialState:"require_verified"|"cold_start"`, `workbookSeed` as an unsigned decimal string or safe integer, `fixedNowUtc` as RFC3339, `timezone:"utc"|"local"|{offsetSeconds}`, `maxElapsedMs`, and `maxWorkUnits`. Exact u64 seeds serialize as strings. Deterministic clock/timezone and positive iteration/time bounds are validated before evaluation; edits validate options even in never/preserving auto mode. Raw EvalConfig and cancellation tokens remain native-only.

The direct binding receives original bytes before createSession/Umya materialization. It returns `Uint8Array` bytes separately from `metadataJson`; the transport never stringifies workbook bytes or recreates summaries from samples. Successful Excel errors retain exact error totals and sampled/truncated locations. Metadata serializes shared upstream `calculation`, `coverage` (complete/partial), `coverageScope`, `cachesInvalidated`, `calculationFlagsChanged`, and `operationDiagnostics`; exact Excel error totals remain separate from bounded operation samples. Complete coverage describes known gaps under the declared assumptions, not numerical equivalence with Excel. `attempted`, `completed`, `artifactAvailable`, and `bytesUnchanged` describe separate facts.

Only typed `CalculationDeferred` with workbook profile and `requireComplete:false` may return unchanged input or valid edited bytes with `coverage:"deferred"`, `attempted:true`, `completed:false`, and a deferred reason/context. Unknown formula/cache/link/diagnostic counts and summary are null. `calculationFlagsChanged` is the upstream known boolean for completed source publication, and null for deferred bytes; skipped edits have no recalculation metadata. Diagnostics include complete `unavailableFunctions` incidence, `unavailableResultCells`, and `workbookFeatures` independently of bounded samples. Logical operation roots, final error results, and cache invalidations are distinct; unavailable legacy error messages remain unknown. Deferred bytes do not certify fresh caches. Strict/requireComplete requests and actual option, corrupt-package, resource, cancellation or output failures remain errors without successful artifacts.

Run the bundled engine tests without installation:

```sh
node --test test/*.test.mjs
```

Local checks do not certify Code, Cowork, browser execution, artifact delivery or publication.
