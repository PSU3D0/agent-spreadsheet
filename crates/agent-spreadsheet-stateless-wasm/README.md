# Stateless WASM size experiment

An unpublished binding crate for comparing the four plugin byte APIs with the
full byte/session runtime. Default features build Variant A; `--no-default-features`
builds Variant B without editing. Neither variant enables rendering or compiles
the full WASM crate's session registry, runtime, storage, or compatibility module.

The production source/recalc, inspection and edit adapters are reused by path.
Their receiver is a zero-sized shim; it owns no sessions, documents or history.
The core crate still compiles its shared implementation modules: canonical
editing uses fork change types and resident calculation-effect types. Actual
retention must be checked in the linked, named optimized module.

Exports are `recalculateSourceXlsx`, `inspectXlsx`, `traceXlsx`, optional
`editXlsx`, `version`, and `capabilities`, plus binding/result helpers.
These preserve the existing plugin development-loader contract. The historical
plugin's registry loader and legacy session transport tests require the full
package; use its explicit development-loader selection for these variants.

Build with the workspace `wasm-release` profile for wasm32-unknown-unknown,
then wasm-bindgen 0.2.114 (`--target web`) and Binaryen 123 `-Oz` using the
same feature flags as scripts/build-wasm-package.js. Keep names in a diagnostic
copy for Twiggy, strip names/debug for payload measurement, and use Brotli Q11.

This crate is an experiment, not a new supported published package.
