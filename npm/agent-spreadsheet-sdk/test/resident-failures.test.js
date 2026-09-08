const assert = require('node:assert/strict')
const test = require('node:test')
const { Worker, MessageChannel } = require('node:worker_threads')
const { Bash } = require('just-bash')
const { connectBindings, createLocalSpreadsheet, TransportError } = require('agent-spreadsheet-sdk')
const { createAspCommand } = require('agent-spreadsheet-sdk/just-bash')

function fixture() {
  const sessions = new Set()
  let next = 0
  return { sessions, bindings: {
    operations: () => ['list_sheets'],
    executeOperation: () => JSON.stringify({ schema_version: '1', operation: 'list_sheets', data: {} }),
    createSession: () => { const id = `session:${++next}`; sessions.add(id); return id },
    sessionMetadata: id => JSON.stringify({ resource_id: id, revision_id: 'revision', durability: 'memory' }),
    disposeSession: id => sessions.delete(id)
  } }
}

test('normal Node worker exit rejects pending and subsequent RPCs', { timeout: 10000 }, async t => {
  const worker = new Worker(`require('node:worker_threads').parentPort.once('message', () => process.exit(0))`, { eval: true })
  t.after(() => worker.terminate())
  const bindings = connectBindings(worker)
  await assert.rejects(bindings.operations(), error => error instanceof TransportError && /completion is unknown/.test(error.message))
  await assert.rejects(bindings.operations(), TransportError)
})

test('MessagePort closure rejects outstanding RPCs', { timeout: 10000 }, async t => {
  const { port1, port2 } = new MessageChannel()
  t.after(() => { port1.close(); port2.close() })
  const pending = connectBindings(port1).operations()
  const rejected = assert.rejects(pending, TransportError)
  port2.close()
  await rejected
})

test('concurrent workbook disposal shares one attempt and a failed attempt is retryable', async () => {
  const { bindings, sessions } = fixture()
  let release, entered
  const gate = new Promise(resolve => { release = resolve })
  const started = new Promise(resolve => { entered = resolve })
  let calls = 0
  bindings.disposeSession = async id => { calls++; entered(); await gate; if (calls === 1) throw new Error('injected failure'); sessions.delete(id) }
  const local = createLocalSpreadsheet({ runtime: bindings })
  const book = await local.open(Uint8Array.of(1))
  const first = book.dispose(), second = book.dispose()
  assert.equal(first, second)
  const outcomes = Promise.allSettled([first, second])
  await started
  assert.equal(calls, 1)
  release()
  assert.deepEqual((await outcomes).map(x => x.status), ['rejected', 'rejected'])
  assert.equal(book.disposed, false)
  await book.dispose()
  assert.equal(calls, 2)
  assert.equal(book.disposed, true)
  assert.equal(sessions.size, 0)
})

for (const hostCleanup of [false, true]) test(`${hostCleanup ? 'host' : 'shell'} cleanup visits every handle and retains failed handles for retry`, async () => {
  const { bindings, sessions } = fixture()
  const dispose = bindings.disposeSession
  let fail = true
  bindings.disposeSession = async id => { if (id === 'session:1' && fail) { fail = false; throw new Error('injected failure') } return dispose(id) }
  const asp = createAspCommand({ bindings })
  const bash = new Bash({ files: { '/book.xlsx': Uint8Array.of(1) }, customCommands: [asp] })
  for (let i = 0; i < 2; i++) assert.equal((await bash.exec('asp session open /book.xlsx')).exitCode, 0)
  if (hostCleanup) await assert.rejects(asp.dispose(), /cleanup/)
  else assert.equal((await bash.exec('asp session close --all')).exitCode, 1)
  assert.deepEqual([...sessions], ['session:1'])
  if (hostCleanup) await asp.dispose()
  else assert.equal((await bash.exec('asp session close --all')).exitCode, 0)
  assert.equal(sessions.size, 0)
  await asp.dispose()
})

test('stateless capability discovery retries after a transient initial failure', async () => {
  const { bindings } = fixture()
  let calls = 0
  bindings.operations = () => { if (++calls === 1) throw new Error('temporarily unavailable'); return ['list_sheets'] }
  const asp = createAspCommand({ bindings })
  const bash = new Bash({ customCommands: [asp] })
  assert.equal((await bash.exec('asp operations')).exitCode, 1)
  const result = await bash.exec('asp operations')
  assert.equal(result.exitCode, 0, result.stderr)
  assert.equal(calls, 2)
  assert.deepEqual(JSON.parse(result.stdout).map(x => x.name), ['list_sheets'])
  await asp.dispose()
})
