const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const test = require('node:test')
const { Bash } = require('just-bash')
const { createAspCommand } = require('agent-spreadsheet-sdk/just-bash')
const { createLocalSpreadsheet } = require('agent-spreadsheet-sdk')

// This file is an explicit generated-runtime gate, never an optional mock test.
const bindings = require(process.env.AGENT_SPREADSHEET_WASM_PACKAGE || (() => { throw new Error('AGENT_SPREADSHEET_WASM_PACKAGE is required') })())
const bytes = fs.readFileSync(path.resolve(__dirname, '../../../crates/agent-spreadsheet/tests/fixtures/f1/baseline.xlsx'))

async function command(bash, command, params, expected = 0) {
  const result = await bash.exec(command, { stdin: params === undefined ? '' : JSON.stringify(params) })
  assert.equal(result.exitCode, expected, JSON.stringify(result))
  return JSON.parse(result.stdout || result.stderr)
}

test('actual just-bash retains Rust owner, reconciles retries, isolates VFS scopes and explicitly exports', async () => {
  const asp = createAspCommand({ bindings })
  const bash = new Bash({ files: { '/source.xlsx': bytes }, customCommands: [asp] })
  const other = new Bash({ files: { '/source.xlsx': bytes }, customCommands: [asp] })
  const opened = await command(bash, 'asp session open /source.xlsx')
  const id = opened.resource_id
  assert.match(id, /^session:/)
  assert.equal(opened.durability, 'memory')
  const prefix = `asp session op ${id}`
  let first
  try {
    for (const value of [3, 7, 2]) {
      const info = await command(bash, `asp session info ${id}`)
      const params = { expected_revision: info.revision_id, mode: 'apply', ops: [{ kind: 'set_cells', sheet_name: 'Sheet1', cells: {
        A1: { kind: 'value', value }, B1: { kind: 'formula', formula: '=A1*3' }
      } }] }
      const written = await command(bash, `${prefix} write --request-id edit-${value}`, params)
      first ??= { params, written }
      await command(bash, `${prefix} recalculate --request-id calc-${value}`, { expected_revision: written.revision_id })
      const read = await command(bash, `${prefix} read_cells`, { sheet_name: 'Sheet1', selection: { kind: 'range', ranges: ['B1'] }, format: 'values' })
      assert.deepEqual(read.data.blocks[0].payload.values, [[value * 3]])
    }
    const info = await command(bash, `asp session info ${id}`)
    assert.equal(info.evaluator.ingests, 1)
    assert.equal(info.evaluator.evaluations, 3)
    assert.equal(info.serializations, 0)
    assert.deepEqual(Buffer.from(await bash.fs.readFileBuffer('/source.xlsx')), bytes)
    assert.deepEqual(await command(bash, `${prefix} write --request-id edit-3`, first.params), first.written)
    await command(bash, `${prefix} write --request-id stale-new-id`, first.params, 1)
    await command(bash, `${prefix} write --request-id bad`, { ...first.params, unknown: true }, 1)
    await command(other, `asp session info ${id}`, undefined, 1)
    await command(other, `asp session op ${id} describe_workbook`, {}, 1)
    await command(bash, `asp session export ${id} --output /saved.xlsx`)
    await command(bash, `asp session export ${id} --output /same.xlsx`)
    assert.deepEqual(await bash.fs.readFileBuffer('/saved.xlsx'), await bash.fs.readFileBuffer('/same.xlsx'))
    assert.equal((await command(bash, `asp session info ${id}`)).serializations, 1)
    await command(bash, `asp session export ${id} --output /saved.xlsx`, undefined, 1)
    const reopened = await command(bash, 'asp session open /saved.xlsx')
    const read = await command(bash, `asp session op ${reopened.resource_id} read_cells`, { sheet_name: 'Sheet1', selection: { kind: 'range', ranges: ['B1'] }, format: 'values' })
    assert.deepEqual(read.data.blocks[0].payload.values, [[6]])
  } finally { await command(bash, 'asp session close --all') }
  await command(bash, `asp session info ${id}`, undefined, 1)
})

test('actual just-bash uses shared history, checkpoints and staged approvals without file round trips', async () => {
  const bash = new Bash({ files: { '/source.xlsx': bytes }, customCommands: [createAspCommand({ bindings })] })
  const { resource_id: id } = await command(bash, 'asp session open /source.xlsx')
  const prefix = `asp session op ${id}`
  const revision = async () => (await command(bash, `asp session info ${id}`)).revision_id
  const readA1 = async () => (await command(bash, `${prefix} read_cells`, { sheet_name: 'Sheet1', selection: { kind: 'range', ranges: ['A1'] }, format: 'values' })).data.blocks[0].payload.values[0][0]
  const edit = async (value, mode = 'apply') => command(bash, `${prefix} write`, { expected_revision: await revision(), mode, ops: [{ kind: 'set_cells', sheet_name: 'Sheet1', cells: { A1: { kind: 'value', value } } }] })
  try {
    await edit(11)
    await edit(22)
    await command(bash, `${prefix} session_history --request-id undo-22`, { action: 'undo', expected_revision: await revision() })
    assert.equal(await readA1(), 11)
    await command(bash, `${prefix} session_history`, { action: 'redo', expected_revision: await revision() })
    assert.equal(await readA1(), 22)
    const list = await command(bash, `${prefix} session_history`, { action: 'list' })
    assert.ok(list.data.records.length >= 4)
    const outcome = await command(bash, `${prefix} session_history`, { action: 'outcome', request_id: 'undo-22' })
    assert.equal(outcome.data.state, 'committed')
    const stage = await edit(33, 'stage')
    assert.equal(await readA1(), 22)
    const stages = await command(bash, `${prefix} staged_change`, { action: 'list' })
    assert.equal(stages.data.staged_changes[0].change_id, stage.data.change_id)
    await command(bash, `${prefix} staged_change`, { action: 'apply', expected_revision: await revision(), change_id: stage.data.change_id })
    assert.equal(await readA1(), 33)
    await command(bash, `${prefix} checkpoint`, { action: 'create', expected_revision: await revision(), label: 'approved' })
    await edit(44)
    const checkpoints = await command(bash, `${prefix} checkpoint`, { action: 'list' })
    assert.equal(checkpoints.data.checkpoints.length, 1)
    await command(bash, `${prefix} checkpoint`, { action: 'restore', expected_revision: await revision(), checkpoint_id: checkpoints.data.checkpoints[0].checkpoint_id })
    assert.equal(await readA1(), 33)
    assert.equal((await command(bash, `asp session info ${id}`)).durability, 'memory')
    assert.deepEqual(Buffer.from(await bash.fs.readFileBuffer('/source.xlsx')), bytes)
  } finally { await command(bash, 'asp session close --all') }
})

test('host disposal drains accepted commands and releases real WASM session slots', async () => {
  let entered, release
  const started = new Promise(resolve => { entered = resolve })
  const gate = new Promise(resolve => { release = resolve })
  const asp = createAspCommand({ bindings: { ...bindings, async executeOperation(...args) {
    entered(); await gate; return bindings.executeOperation(...args)
  } } })
  const bash = new Bash({ files: { '/source.xlsx': bytes }, customCommands: [asp] })
  const opened = await command(bash, 'asp session open /source.xlsx')
  const pending = command(bash, `asp session op ${opened.resource_id} describe_workbook`, {})
  await started
  let disposed = false
  const closing = asp.dispose().then(() => { disposed = true })
  await Promise.resolve()
  assert.equal(disposed, false)
  release()
  await pending
  await closing
  assert.throws(() => bindings.sessionMetadata(opened.resource_id))
  await command(bash, 'asp session open /source.xlsx', undefined, 1)
  await asp.dispose()
  for (let i = 0; i < 35; i++) {
    const cmd = createAspCommand({ bindings })
    const shell = new Bash({ files: { '/source.xlsx': bytes }, customCommands: [cmd] })
    await command(shell, 'asp session open /source.xlsx')
    await cmd.dispose()
  }
})

for (const worker of [false, true]) test(`SDK canonical retries and metadata use the actual ${worker ? 'worker' : 'local'} WASM owner`, async () => {
  const local = createLocalSpreadsheet({ runtime: worker ? { module: process.env.AGENT_SPREADSHEET_WASM_PACKAGE } : bindings, worker })
  const book = await local.open(bytes)
  try {
    const metadata = await book.metadata()
    const input = { resource_id: book.resourceId, expected_revision: metadata.revision_id, mode: 'apply', ops: [{ kind: 'set_cells', sheet_name: 'Sheet1', cells: { A1: { kind: 'value', value: 91 } } }] }
    const first = await local.canonical.execute('write', input, { requestId: 'sdk-edit' })
    assert.deepEqual(await local.canonical.execute('write', input, { requestId: 'sdk-edit' }), first)
    await assert.rejects(local.canonical.execute('write', input, { requestId: '' }), /1–256/)
  } finally { await book.dispose(); await local.close() }
})
