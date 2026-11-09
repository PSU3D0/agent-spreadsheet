# Synthetic host check

Use only synthetic workbooks. Local Node tests do not establish support or artifact delivery in another host.

1. Locate the copied skill folder and check Node >=20.
2. Run `node scripts/xlsx.mjs doctor`; retain the version and verified hashes.
3. Use `syntheticWorkbook` from scripts/synthetic.mjs to write a small workbook in the host's writable artifact folder. Supply an intentionally stale B1 cache with formula A1*2 and A1=21.
4. Run source-recalculate with an explicit output destination. Inspect A1:B1 and trace Sheet1!B1 with JSON requests as described in stateless-inspection.md. Apply a batch edit as described in stateless-edits.md.
5. Attach the output workbook and JSON receipts through the host's artifact UI. Download and open the workbook; record actual delivery separately from engine results.

Record host/app version, date, Node version, doctor output, exact commands, calculation state, permissions and artifact delivery. The bundled engine needs no installation or network. Do not disable host protections. The compiled Brotli engine is disclosed for manual directory review.
