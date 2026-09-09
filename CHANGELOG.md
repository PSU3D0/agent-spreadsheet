# Changelog

## Unreleased

- Migrated the document and rendering layers to published Umya 3.1 and Formualizer 0.9.1, removing the unpublished Umya Git-patch dependency.
- Added shared cold-import and export repair for upstream border-colour loss and colour-selector hash collisions. Theme/indexed/RGB identity is retained; no workbook reimport or serialization is added to warm edit/recalculate loops.

- Added shared resident spreadsheet sessions across native CLI/MCP and the WASM SDK, retaining the document and Formualizer evaluator during ordinary edit/recalculate/read loops.
- Added genuine just-bash resident commands with VFS-scoped handles, explicit XLSX/artifact export, request reconciliation, history, checkpoints, staged approvals and host cleanup.
- Added native journal-backed ownership, automatic private-host startup, revision CAS, original request outcomes and generation-pinned export recovery.
- Added SDK request-ID forwarding and authoritative session metadata for local and worker-backed execution.
- Fixed worker-exit handling, concurrent and failed session cleanup, and retry after transient capability-discovery failure.
- Fixed resident calculation diagnostics and actual evaluation counts, file-command CAS generations, and typed formula-cache fidelity.
- Kept schema validation in Rust and retained renderer capabilities; adjusted the WASM size policy for the expanded runtime.
- Limited native release targets to Linux and macOS. Windows binaries and legacy session migration are not included in this release.
- Documented that portable sessions retain history only in memory: XLSX exports save the document, not a restartable journal. Formualizer is the release-critical backend; existing optional LibreOffice support is not the focus of this release's validation.
