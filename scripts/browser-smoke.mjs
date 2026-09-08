#!/usr/bin/env node
// Exercise the published web WASM loader and built SDK, directly and in a real Web Worker.
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { createRequire } from 'node:module'
import { readFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const { chromium } = createRequire(path.join(root, 'npm/agent-spreadsheet-sdk/package.json'))('playwright-core')
const server = createServer(async (req, res) => {
  try {
    const pathname = new URL(req.url, 'http://localhost').pathname
    if (pathname === '/') { res.end('<!doctype html><title>Spreadsheet runtime smoke</title>'); return }
    if (pathname === '/worker.js') {
      res.setHeader('Content-Type', 'text/javascript')
      res.end(`import { serveBindings } from '/npm/agent-spreadsheet-sdk/dist/esm/worker.js'; import { createWasmRuntime } from '/npm/agent-spreadsheet-wasm/src/index.js'; serveBindings(self, createWasmRuntime());`)
      return
    }
    const permitted = ['/npm/agent-spreadsheet-wasm/', '/npm/agent-spreadsheet-sdk/dist/esm/', '/crates/agent-spreadsheet/tests/fixtures/f1/']
    if (!permitted.some(prefix => pathname.startsWith(prefix))) { res.writeHead(404).end(); return }
    const filename = path.resolve(root, `.${pathname}`)
    if (!filename.startsWith(`${root}${path.sep}`)) { res.writeHead(403).end(); return }
    res.setHeader('Content-Type', filename.endsWith('.wasm') ? 'application/wasm' : filename.endsWith('.js') ? 'text/javascript' : 'application/octet-stream')
    res.end(await readFile(filename))
  } catch { res.writeHead(404).end() }
})
let browser, timer
try {
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve) })
  browser = await chromium.launch({ executablePath: process.env.BROWSER_BIN || process.env.CHROME_BIN || '/usr/bin/google-chrome', headless: true, args: ['--no-sandbox'] })
  const page = await browser.newPage()
  await page.goto(`http://127.0.0.1:${server.address().port}`)
  const results = await Promise.race([
    new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('browser smoke timed out')), 120000) }),
    page.evaluate(async () => {
      const { createWasmRuntime } = await import('/npm/agent-spreadsheet-wasm/src/index.js')
      const { createLocalSpreadsheet } = await import('/npm/agent-spreadsheet-sdk/dist/esm/index.js')
      const response = await fetch('/crates/agent-spreadsheet/tests/fixtures/f1/baseline.xlsx')
      if (!response.ok) throw new Error('fixture fetch failed')
      const bytes = new Uint8Array(await response.arrayBuffer())
      const results = []
      for (const worker of [false, true]) {
        const local = createLocalSpreadsheet({ runtime: worker ? { module: `${location.origin}/npm/agent-spreadsheet-wasm/src/index.js` } : await createWasmRuntime(), worker: worker ? { url: `${location.origin}/worker.js` } : false })
        const book = await local.open(bytes)
        try {
          let first
          for (const value of [3, 7, 2]) {
            const metadata = await book.metadata()
            const input = { resource_id: book.resourceId, expected_revision: metadata.revision_id, mode: 'apply', ops: [{ kind: 'set_cells', sheet_name: 'Sheet1', cells: { A1: { kind: 'value', value }, B1: { kind: 'formula', formula: '=A1*3' } } }] }
            const written = await local.canonical.execute('write', input, { requestId: 'edit-' + value })
            first ??= { input, written }
            await local.canonical.execute('recalculate', { resource_id: book.resourceId, expected_revision: written.revision_id })
            const read = await book.readCells({ sheet_name: 'Sheet1', selection: { kind: 'range', ranges: ['B1'] }, format: 'values' })
            if (read.data.blocks[0].payload.values[0][0] !== value * 3) throw new Error('numeric oracle failed')
          }
          const warm = await book.metadata()
          if (warm.evaluator.ingests !== 1 || warm.evaluator.evaluations !== 3 || warm.serializations !== 0) throw new Error('retention counters failed: ' + JSON.stringify(warm))
          const retried = await local.canonical.execute('write', first.input, { requestId: 'edit-3' })
          if (JSON.stringify(retried) !== JSON.stringify(first.written)) throw new Error('original retry changed')
          const png = await book.renderSheet({ sheet_name: 'Sheet1', range: 'A1:C3' })
          if (png.png[0] !== 137 || png.png[1] !== 80) throw new Error('PNG signature failed')
          const exported = await book.exportBytes()
          if (exported[0] !== 80 || exported[1] !== 75) throw new Error('XLSX signature failed')
          results.push({ worker, warm, exportBytes: exported.length, pngBytes: png.png.length })
        } finally { await book.dispose(); await local.close() }
      }
      return results
    })
  ])
  assert.equal(results.length, 2)
  console.log(JSON.stringify({ browser: browser.version(), results }, null, 2))
} finally {
  clearTimeout(timer)
  await browser?.close()
  server.closeAllConnections()
  await new Promise(resolve => server.close(resolve))
}
