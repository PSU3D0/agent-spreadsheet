# Legacy resident migration text fixtures

Events and branch/HEAD projections were produced by the actual agent-spreadsheet CLI at commit d3d37c7 using public synthetic Sheet1 cell edits. They are copied unchanged from the parent-generated linear-undone and divergent fixtures; hashes and append linkage remain genuine. No workbook archive, cached snapshot, filesystem path, or staged payload is included.

The integration test generates a fresh Umya base (A1=1, B1=A1*2), explicitly binds its hash in the frozen import manifest, and generates a rejected legacy approval identity. Legacy event hashes bind event payloads, not XLSX ZIP bytes. Thus this is intentional test metadata rebinding, not a claim that generated ZIP bytes match the original fixture. The original full CLI fixtures remain external, immutable local evidence.
