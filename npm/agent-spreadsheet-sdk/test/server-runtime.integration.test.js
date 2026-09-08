// Server runtime integration against a spawned agent-spreadsheet-mcp process.
//
//   ASP_BINARY=../../target/debug/asp \
//   SPREADSHEET_MCP_BINARY=../../target/debug/agent-spreadsheet-mcp node --test \
//     test/server-runtime.integration.test.js
//
// Runs real native owners in an owned private temporary root. Both built binaries
// are required; Linux fault cases hold the journal's actual OS lock with flock.

const test = require("node:test")
const assert = require("node:assert/strict")
const { spawn } = require("node:child_process")
const fs = require("node:fs")
const net = require("node:net")
const os = require("node:os")
const path = require("node:path")

const {
  CanonicalOperationError,
  connectSpreadsheetServer
} = require("agent-spreadsheet-sdk")

const binary = process.env.SPREADSHEET_MCP_BINARY
const fixture = path.resolve(
  __dirname, "..", "..", "..",
  "crates", "agent-spreadsheet", "tests", "fixtures", "f1", "baseline.xlsx"
)

async function hostRequest(root, payload, status = 200) {
  const crypto = require("node:crypto")
  const discovery = JSON.parse(fs.readFileSync(path.join(root, "discovery.json")))
  const body = Buffer.from(JSON.stringify(payload))
  const nonce = crypto.randomBytes(16).toString("hex"), time = Math.floor(Date.now() / 1000)
  const metadata = Buffer.alloc(8); metadata.writeBigUInt64BE(BigInt(time))
  const mac = crypto.createHmac("sha256", discovery.credential)
  for (const part of [Buffer.from("asp-resident-http-v1"), Buffer.from("request"), Buffer.from(nonce), metadata, body]) {
    const size = Buffer.alloc(8); size.writeBigUInt64BE(BigInt(part.length)); mac.update(size); mac.update(part)
  }
  const response = await fetch(`http://127.0.0.1:${discovery.port}/`, { method: "POST", body, headers: {
    "content-type": "application/json", "x-asp-nonce": nonce, "x-asp-time": String(time), "x-asp-signature": mac.digest("hex")
  } })
  assert.equal(response.status, status, await response.clone().text())
  return response.json()
}

function freePort() {
  return new Promise((resolve, reject) => {
    const probe = net.createServer()
    probe.on("error", reject)
    probe.listen(0, "127.0.0.1", () => {
      const { port } = probe.address()
      probe.close(() => resolve(port))
    })
  })
}

async function waitForRoute(baseUrl, deadlineMs = 30_000) {
  const started = Date.now()
  for (;;) {
    try {
      const response = await fetch(`${baseUrl}/v1/operations`)
      if (response.ok) return
    } catch {
      // not listening yet
    }
    if (Date.now() - started > deadlineMs) throw new Error(`${baseUrl} never came up`)
    await new Promise((resolve) => setTimeout(resolve, 150))
  }
}

test("server runtime drives a live canonical /v1 route", async (t) => {
  assert.ok(binary && fs.existsSync(binary), "SPREADSHEET_MCP_BINARY must name the built MCP executable")
  const workspace = fs.mkdtempSync(path.join(os.tmpdir(), "asp-sdk-server-"))
  fs.copyFileSync(fixture, path.join(workspace, "baseline.xlsx"))
  fs.copyFileSync(path.resolve(__dirname,"..","..","..","crates","agent-spreadsheet-mcp","tests","test_files","vba_minimal.xlsm"),path.join(workspace,"macro.xlsm"))
  const residentRoot = path.join(workspace, "resident")
  fs.mkdirSync(residentRoot, { mode: 0o700 })
  fs.mkdirSync(path.join(residentRoot, "resources"), { mode: 0o700 })
  const port = await freePort()
  const standalone = path.join(workspace, "standalone-mcp")
  fs.copyFileSync(binary, standalone)
  fs.chmodSync(standalone, 0o700)
  const child = spawn(standalone, [], {
    env: {
      ...process.env,
      SPREADSHEET_MCP_WORKSPACE: workspace,
      ASP_RESIDENT_ROOT: residentRoot,
      SPREADSHEET_MCP_TRANSPORT: "http",
      SPREADSHEET_MCP_HTTP_BIND: `127.0.0.1:${port}`,
      SPREADSHEET_MCP_RECALC_ENABLED: "true",
      SPREADSHEET_MCP_VBA_ENABLED: "true"
    },
    stdio: ["ignore", "pipe", "pipe"]
  })
  t.after(async () => {
    child.kill("SIGTERM")
    if (fs.existsSync(path.join(residentRoot, "discovery.json"))) {
      await hostRequest(residentRoot, { control: "shutdown" })
      await new Promise(resolve => setTimeout(resolve, 300))
    }
    fs.rmSync(workspace, { recursive: true, force: true })
  })

  const baseUrl = `http://127.0.0.1:${port}`
  await waitForRoute(baseUrl)
  let sequence = 0
  const client = connectSpreadsheetServer({ baseUrl, fetch: (url, init = {}) => fetch(url, {
    ...init, headers: { ...init.headers, "x-agent-spreadsheet-request-id": `http-${++sequence}` }
  }) })

  const capabilities = await client.capabilities()
  assert.ok(capabilities.includes("create_fork"))
  assert.ok(capabilities.includes("read_cells"))

  const listed = await client.listWorkbooks({})
  assert.equal(listed.operation, "list_workbooks")
  const [descriptor] = listed.data.workbooks
  assert.match(descriptor.resource_id, /^wb:/)

  const workbook = client.workbook(descriptor.resource_id)

  await t.test("ordinary SDK callers create and mutate without custom identity metadata", async () => {
    const plain = connectSpreadsheetServer({baseUrl})
    const source = plain.workbook(descriptor.resource_id)
    const first = await source.createFork()
    const second = await source.createFork()
    assert.notEqual(first.resourceId,second.resourceId,"missing-ID calls are distinct operations")
    await first.write({mode:"apply",ops:[{kind:"set_cells",sheet_name:"Sheet1",cells:{A1:{kind:"value",value:91}}}]})
    const read = await first.readCells({sheet_name:"Sheet1",selection:{kind:"range",ranges:["A1"]},format:"values"})
    assert.equal(read.data.blocks[0].payload.values[0][0],91)
    const args = {resource_id:first.resourceId,expected_revision:read.revision_id,mode:"apply",ops:[{kind:"set_cells",sheet_name:"Sheet1",cells:{A1:{kind:"value",value:92}}}]}
    const response = await fetch(`${baseUrl}/v1/op/write`,{method:"POST",headers:{"content-type":"application/json"},body:JSON.stringify(args)})
    assert.equal(response.status,200,await response.clone().text())
    const chosen = response.headers.get("x-agent-spreadsheet-request-id")
    assert.match(chosen,/^[0-9a-f-]{36}$/)
    const original = await response.json()
    const replay = await fetch(`${baseUrl}/v1/op/write`,{method:"POST",headers:{"content-type":"application/json","x-agent-spreadsheet-request-id":chosen},body:JSON.stringify(args)})
    assert.equal(replay.status,200)
    assert.equal(replay.headers.get("x-agent-spreadsheet-request-id"),chosen)
    assert.deepEqual(await replay.json(),original)
    await first.readCells({sheet_name:"Sheet1",selection:{kind:"range",ranges:["A1"]},format:"values"}) // refresh CAS
    await first.discard()
    await second.discard()
  })

  await t.test("the read surface returns whole envelopes", async () => {
    const described = await workbook.describe()
    assert.equal(described.operation, "describe_workbook")
    assert.equal(described.resource_id, workbook.resourceId)
    assert.equal(typeof described.revision_id, "string")
    assert.equal(workbook.revisionId, described.revision_id)

    const sheets = await workbook.listSheets()
    assert.ok(Array.isArray(sheets.data.sheets))
  })

  await t.test("a fork writes, reads back, and reports its changes", async () => {
    const fork = await workbook.createFork()
    assert.match(fork.resourceId, /^fork:/)

    const written = await fork.write({
      mode: "apply",
      ops: [{
        kind: "set_cells",
        sheet_name: "Sheet1",
        cells: { A1: { kind: "value", value: "e" } }
      }]
    })
    assert.equal(written.data.status, "applied")
    assert.equal(fork.revisionId, written.revision_id)

    const read = await fork.readCells({
      sheet_name: "Sheet1",
      selection: { kind: "range", ranges: ["A1:A1"] },
      format: "dense"
    })
    assert.equal(read.operation, "read_cells")
    assert.match(JSON.stringify(read.data), /"e"/)

    const changes = await fork.getChanges({ view: { kind: "operations" } })
    assert.equal(changes.operation, "get_changes")

    const created = await fork.checkpoint({ action: "create", label: "after-write" })
    assert.equal(created.operation, "checkpoint")
    const listedCheckpoints = await fork.checkpoint({ action: "list" })
    assert.ok(JSON.stringify(listedCheckpoints.data).includes("after-write"))

    const verified = await fork.verifyAgainst(workbook, {
      targets: ["Sheet1!A1"],
      targets_only: true
    })
    assert.equal(verified.operation, "verify_workbook")

    const discarded = await fork.discard()
    assert.equal(discarded.operation, "discard_fork")
    assert.equal(fork.discarded, true)
  })

  await t.test("native lifecycle, optional snapshots, artifacts and restart reconciliation", async () => {
    const call = async (operation, args, identity = `extended-${++sequence}`) => {
      const response = await fetch(`${baseUrl}/v1/op/${operation}`, { method: "POST", headers: {
        "content-type": "application/json", "x-agent-spreadsheet-request-id": identity
      }, body: JSON.stringify(args) })
      assert.equal(response.status, 200, await response.clone().text())
      return response.json()
    }
    const base = await workbook.describe()
    const creationArgs = { resource_id: workbook.resourceId, expected_revision: base.revision_id }
    const creation = await call("create_fork", creationArgs, "extended-create")
    const id = creation.resource_id
    const writeArgs = { resource_id: id, expected_revision: creation.revision_id, mode: "apply", atomic: true, ops: [
      { kind: "set_cells", sheet_name: "Sheet1", cells: { A1: { kind: "value", value: 8 }, B1: { kind: "formula", formula: "A1*3" } } }
    ] }
    const written = await call("write", writeArgs, "extended-write")
    let current = await call("recalculate", { resource_id: id, expected_revision: written.revision_id })
    const read = await call("read_cells", { resource_id: id, sheet_name: "Sheet1", selection: {kind:"range",ranges:["B1"]},format:"values" })
    assert.equal(read.data.blocks[0].payload.values[0][0], 24)
    assert.equal(read.revision_id, current.revision_id)
    const ping = await hostRequest(residentRoot,{control:"ping"})
    if (process.platform === "linux") assert.equal(fs.readlinkSync(`/proc/${ping.pid}/exe`), standalone)
    const cli = process.env.ASP_BINARY
    assert.ok(cli && fs.existsSync(cli), "ASP_BINARY must name the built CLI for interoperability proof")
    const cliRead = require("node:child_process").spawnSync(cli,["op","read_cells","--bind",id,"--request-id","http-cli-read","--json",JSON.stringify({sheet_name:"Sheet1",selection:{kind:"range",ranges:["B1"]},format:"values"})],{cwd:workspace,env:{...process.env,ASP_RESIDENT_ROOT:residentRoot},encoding:"utf8"})
    assert.equal(cliRead.status,0,cliRead.stderr)
    assert.deepEqual(JSON.parse(cliRead.stdout),read)
    const cliCreate = require("node:child_process").spawnSync(cli,["op","create_fork","--request-id","cli-created-through-mcp-host","--json",JSON.stringify(creationArgs)],{cwd:workspace,env:{...process.env,ASP_RESIDENT_ROOT:residentRoot},encoding:"utf8"})
    assert.equal(cliCreate.status,0,cliCreate.stderr)
    const cliCreated = JSON.parse(cliCreate.stdout)
    const crossWrite = await call("write",{resource_id:cliCreated.resource_id,expected_revision:cliCreated.revision_id,mode:"apply",ops:[{kind:"set_cells",sheet_name:"Sheet1",cells:{A1:{kind:"value",value:77}}}]})
    const crossRead = await call("read_cells",{resource_id:cliCreated.resource_id,sheet_name:"Sheet1",selection:{kind:"range",ranges:["A1"]},format:"values"})
    assert.equal(crossRead.data.blocks[0].payload.values[0][0],77)
    assert.equal(crossRead.revision_id,crossWrite.revision_id)
    await call("discard_fork",{resource_id:cliCreated.resource_id,expected_revision:crossRead.revision_id})
    const conflict = await fetch(`${baseUrl}/v1/op/write`,{method:"POST",headers:{"content-type":"application/json","x-agent-spreadsheet-request-id":"extended-write"},body:JSON.stringify({...writeArgs,atomic:false})})
    assert.equal(conflict.status,400,await conflict.clone().text())
    assert.match((await conflict.json()).error.message,/request identity reuse/)
    const timeoutPort = await freePort()
    const timeoutServer = spawn(standalone,[],{env:{...process.env,ASP_RESIDENT_ROOT:residentRoot,SPREADSHEET_MCP_WORKSPACE:workspace,SPREADSHEET_MCP_TRANSPORT:"http",SPREADSHEET_MCP_HTTP_BIND:`127.0.0.1:${timeoutPort}`,SPREADSHEET_MCP_RECALC_ENABLED:"true",SPREADSHEET_MCP_VBA_ENABLED:"true",SPREADSHEET_MCP_TOOL_TIMEOUT_MS:"1",SPREADSHEET_MCP_ENABLED_TOOLS:"write,session_history"},stdio:["ignore","pipe","pipe"]})
    try {
      const timeoutBase = `http://127.0.0.1:${timeoutPort}`
      await waitForRoute(timeoutBase)
      const forbidden = await fetch(`${timeoutBase}/v1/op/create_fork`,{method:"POST",headers:{"content-type":"application/json","x-agent-spreadsheet-request-id":"must-not-create"},body:JSON.stringify(creationArgs)})
      assert.equal(forbidden.status,501)
      assert.equal((await forbidden.json()).error.code,"CAPABILITY_UNAVAILABLE")
      const timedArgs = {resource_id:id,expected_revision:current.revision_id,mode:"apply",atomic:true,ops:[{kind:"set_cells",sheet_name:"Sheet1",cells:{A1:{kind:"value",value:10}}}]}
      const key = id.split(":")[1]
      const journalLock = path.join(residentRoot,"resources",key,"journal",`${key}.lock`)
      const lockHolder = spawn("flock",["--exclusive",journalLock,"sh","-c","printf ready; read release"],{stdio:["pipe","pipe","pipe"]})
      const lockExited = new Promise(resolve=>lockHolder.once("exit",resolve))
      let independent
      try {
        await new Promise((resolve,reject)=>{lockHolder.once("error",reject);lockHolder.stdout.once("data",resolve)})
        const completed = await fetch(`${timeoutBase}/v1/op/write`,{method:"POST",headers:{"content-type":"application/json","x-agent-spreadsheet-request-id":"one-ms-accepted"},body:JSON.stringify(timedArgs)})
        assert.equal(completed.status,503,await completed.clone().text())
        const unknown = await completed.json()
        assert.equal(unknown.error.code,"OUTCOME_UNKNOWN")
        assert.match(unknown.error.message,/one-ms-accepted/)
        let admission
        for(let attempt=0;attempt<100;attempt++) {
          admission = await hostRequest(residentRoot,{control:"admission",resource_id:id})
          if(admission.active_request_id==="one-ms-accepted") break
          await new Promise(resolve=>setTimeout(resolve,10))
        }
        assert.equal(admission.active_request_id,"one-ms-accepted",JSON.stringify(admission))
        independent = await call("create_fork",creationArgs,"timeout-independent")
        const independentWrite = await call("write",{resource_id:independent.resource_id,expected_revision:independent.revision_id,mode:"apply",ops:[{kind:"set_cells",sheet_name:"Sheet1",cells:{A1:{kind:"value",value:66}}}]})
        const independentRead = await call("read_cells",{resource_id:independent.resource_id,sheet_name:"Sheet1",selection:{kind:"range",ranges:["A1"]},format:"values"})
        assert.equal(independentRead.data.blocks[0].payload.values[0][0],66)
        assert.equal(independentRead.revision_id,independentWrite.revision_id)
        independent = independentRead
      } finally { lockHolder.stdin.end("release\n"); await lockExited }
      const completedBody = await call("write",timedArgs,"one-ms-accepted")
      assert.notEqual(completedBody.revision_id,current.revision_id)
      current = completedBody
      const timedOutcome = await call("session_history",{action:"outcome",resource_id:id,request_id:"one-ms-accepted"})
      assert.deepEqual(timedOutcome.data.response,completedBody)
      const history = await call("session_history",{action:"list",resource_id:id})
      assert.equal(history.data.records.filter(record=>record.request_id==="one-ms-accepted").length,1)
      await call("discard_fork",{resource_id:independent.resource_id,expected_revision:independent.revision_id})
      // Disconnect only after non-workbook admission metadata proves that
      // the owner accepted this exact request, while its journal stays locked.
      const lostLock = spawn("flock",["--exclusive",journalLock,"sh","-c","printf ready; read release"],{stdio:["pipe","pipe","pipe"]})
      const lostLockExited = new Promise(resolve=>lostLock.once("exit",resolve))
      await new Promise((resolve,reject)=>{lostLock.once("error",reject);lostLock.stdout.once("data",resolve)})
      const lostArgs = {...timedArgs,expected_revision:current.revision_id,ops:[{kind:"set_cells",sheet_name:"Sheet1",cells:{A1:{kind:"value",value:11}}}]}
      const body = JSON.stringify(lostArgs)
      const socket = net.createConnection({host:"127.0.0.1",port:timeoutPort})
      socket.pause()
      await new Promise((resolve,reject)=>{socket.once("error",reject);socket.once("connect",()=>socket.write(`POST /v1/op/write HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nX-Agent-Spreadsheet-Request-Id: lost-http-response\r\nContent-Length: ${Buffer.byteLength(body)}\r\nConnection: close\r\n\r\n${body}`,resolve))})
      try {
        let admission
        for(let attempt=0;attempt<100;attempt++) {
          admission = await hostRequest(residentRoot,{control:"admission",resource_id:id})
          if(admission.active_request_id==="lost-http-response") break
          await new Promise(resolve=>setTimeout(resolve,10))
        }
        assert.equal(admission.active_request_id,"lost-http-response",JSON.stringify(admission))
        socket.destroy()
      } finally { socket.destroy(); lostLock.stdin.end("release\n"); await lostLockExited }
      const reconciled = await call("write",lostArgs,"lost-http-response")
      const outcome = await call("session_history",{action:"outcome",resource_id:id,request_id:"lost-http-response"})
      assert.equal(outcome.data.state,"committed")
      assert.deepEqual(reconciled,outcome.data.response)
      assert.notEqual(reconciled.revision_id,current.revision_id)
      current = reconciled
      const afterLoss = await call("read_cells",{resource_id:id,sheet_name:"Sheet1",selection:{kind:"range",ranges:["A1"]},format:"values"})
      assert.equal(afterLoss.data.blocks[0].payload.values[0][0],11)
      assert.equal(afterLoss.revision_id,current.revision_id)
      // Missing-ID callers also learn the chosen identity in a bounded unknown
      // response. This does not promise safety if that response itself is lost.
      const generatedArgs = {...timedArgs,expected_revision:current.revision_id,ops:[{kind:"set_cells",sheet_name:"Sheet1",cells:{A1:{kind:"value",value:12}}}]}
      const generatedLock = spawn("flock",["--exclusive",journalLock,"sh","-c","printf ready; read release"],{stdio:["pipe","pipe","pipe"]})
      const generatedExited = new Promise(resolve=>generatedLock.once("exit",resolve))
      let generatedIdentity
      try {
        await new Promise((resolve,reject)=>{generatedLock.once("error",reject);generatedLock.stdout.once("data",resolve)})
        const unknown = await fetch(`${timeoutBase}/v1/op/write`,{method:"POST",headers:{"content-type":"application/json"},body:JSON.stringify(generatedArgs)})
        assert.equal(unknown.status,503,await unknown.clone().text())
        generatedIdentity = unknown.headers.get("x-agent-spreadsheet-request-id")
        assert.match(generatedIdentity,/^[0-9a-f-]{36}$/)
        const body = await unknown.json()
        assert.equal(body.error.code,"OUTCOME_UNKNOWN")
        assert.ok(body.error.message.includes(generatedIdentity))
        let admission
        for(let attempt=0;attempt<100;attempt++) {
          admission = await hostRequest(residentRoot,{control:"admission",resource_id:id})
          if(admission.active_request_id===generatedIdentity) break
          await new Promise(resolve=>setTimeout(resolve,10))
        }
        assert.equal(admission.active_request_id,generatedIdentity)
      } finally { generatedLock.stdin.end("release\n"); await generatedExited }
      current = await call("write",generatedArgs,generatedIdentity)
      const generatedOutcome = await call("session_history",{action:"outcome",resource_id:id,request_id:generatedIdentity})
      assert.deepEqual(generatedOutcome.data.response,current)
    } finally { timeoutServer.kill("SIGTERM"); await new Promise(resolve=>timeoutServer.once("exit",resolve)) }
    const counters = await hostRequest(residentRoot, {control:"diagnostics",resource_id:id})
    assert.equal(counters.ingests, 1); assert.equal(counters.serializations, 0)
    const stageArgs = {resource_id:id,expected_revision:current.revision_id,mode:"stage",atomic:true,ops:[
      {kind:"set_cells",sheet_name:"Sheet1",cells:{A1:{kind:"value",value:9}}}
    ]}
    const staged = await call("write", stageArgs, "extended-stage")
    assert.equal(staged.revision_id, current.revision_id)
    const checkpointArgs = {action:"create",resource_id:id,expected_revision:current.revision_id,label:"http-snapshot"}
    const checkpoint = await call("checkpoint",checkpointArgs,"extended-checkpoint")
    assert.equal(checkpoint.revision_id,current.revision_id)
    const stagedList = await call("staged_change",{action:"list",resource_id:id})
    const change = stagedList.data.staged_changes[0].change_id
    current = await call("staged_change",{action:"apply",resource_id:id,expected_revision:current.revision_id,change_id:change})
    current = await call("recalculate",{resource_id:id,expected_revision:current.revision_id})
    const warmAfterApply = await hostRequest(residentRoot,{control:"diagnostics",resource_id:id})
    assert.equal(warmAfterApply.ingests,1)
    assert.equal(warmAfterApply.evaluations,2)
    assert.equal(warmAfterApply.serializations,0)
    const child = await call("create_fork",{resource_id:id,expected_revision:current.revision_id},"extended-child")
    const verified = await call("verify_workbook",{resource_id:id,baseline_resource_id:child.resource_id,targets:["Sheet1!B1"]})
    assert.equal(verified.data.summary.changed_targets,0)
    assert.equal(verified.revision_id,current.revision_id)
    const screenshot = await call("screenshot_sheet",{resource_id:id,sheet_name:"Sheet1",range:"A1:B2"})
    assert.equal(screenshot.revision_id,current.revision_id)
    const png = await fetch(`${baseUrl}/v1/artifacts/${encodeURIComponent(screenshot.data.artifact.handle)}`)
    assert.equal(png.status,200); assert.equal(png.headers.get("content-type"),"image/png")
    const candidates = await call("sheetport_manifest",{action:"candidates",resource_id:id})
    const sheetport = await call("execute_sheetport",{resource_id:id,manifest_yaml:candidates.data.manifest_yaml,inputs:{}})
    assert.equal(sheetport.operation,"execute_sheetport")
    const exportArgs = {resource_id:id,expected_revision:current.revision_id,destination:{kind:"workspace",name:"http-export.xlsx"}}
    const exported = await call("export_fork",exportArgs,"extended-export")
    const download = await fetch(`${baseUrl}/v1/artifacts/${exported.data.artifact.artifact_id}`)
    assert.equal(download.status,200)
    const bytes = Buffer.from(await download.arrayBuffer())
    assert.equal(bytes.length,exported.data.artifact.bytes)
    assert.equal(require("node:crypto").createHash("sha256").update(bytes).digest("hex"),exported.data.artifact.sha256)
    const exportedPath = path.join(workspace,"artifacts",`${exported.data.artifact.sha256}.xlsx`)
    const artifactRead = require("node:child_process").spawnSync(cli,["op","read_cells","--bind",exportedPath,"--json",JSON.stringify({sheet_name:"Sheet1",selection:{kind:"range",ranges:["B1"]},format:"values"})],{encoding:"utf8"})
    assert.equal(artifactRead.status,0,artifactRead.stderr)
    assert.equal(JSON.parse(artifactRead.stdout).data.blocks[0].payload.values[0][0],27)
    assert.deepEqual(fs.readFileSync(path.join(workspace,"baseline.xlsx")),fs.readFileSync(fixture))
    await hostRequest(residentRoot,{control:"shutdown"})
    await new Promise(resolve=>setTimeout(resolve,300))
    assert.deepEqual(await call("create_fork",creationArgs,"extended-create"),creation)
    const inactive = await call("list_forks",{})
    assert.equal(inactive.data.forks.find(fork=>fork.resource_id===id).revision_id,null)
    assert.deepEqual(await call("write",writeArgs,"extended-write"),written)
    assert.deepEqual(await call("write",stageArgs,"extended-stage"),staged)
    assert.deepEqual(await call("checkpoint",checkpointArgs,"extended-checkpoint"),checkpoint)
    assert.deepEqual(await call("export_fork",exportArgs,"extended-export"),exported)
    let live = await call("describe_workbook",{resource_id:id})
    assert.notEqual(live.revision_id,current.revision_id)
    live = await call("recalculate",{resource_id:id,expected_revision:live.revision_id})
    const recovered = await call("read_cells",{resource_id:id,sheet_name:"Sheet1",selection:{kind:"range",ranges:["B1"]},format:"values"})
    assert.equal(recovered.data.blocks[0].payload.values[0][0],27)
    assert.equal(recovered.revision_id,live.revision_id)
    const discardArgs = {resource_id:id,expected_revision:live.revision_id}
    const discarded = await call("discard_fork",discardArgs,"extended-discard")
    assert.deepEqual(await call("discard_fork",discardArgs,"extended-discard"),discarded)
    assert.deepEqual(await call("export_fork",exportArgs,"extended-export"),exported)
    const childLive = await call("describe_workbook",{resource_id:child.resource_id})
    await call("discard_fork",{resource_id:child.resource_id,expected_revision:childLive.revision_id})
  })

  await t.test("terminal owners drain beyond the live-owner cap and retry cold", async () => {
    const call = async (operation, arguments_, identity) => {
      const response = await fetch(`${baseUrl}/v1/op/${operation}`, { method: "POST", headers: {
        "content-type": "application/json", "x-agent-spreadsheet-request-id": identity
      }, body: JSON.stringify(arguments_) })
      assert.equal(response.status, 200, await response.clone().text())
      return response.json()
    }
    const source = await workbook.describe()
    for (let index = 0; index < 35; index++) {
      const args = { resource_id: workbook.resourceId, expected_revision: source.revision_id }
      const creation = await call("create_fork", args, `cycle-create-${index}`)
      const resource = creation.resource_id
      const discardArgs = { resource_id: resource, expected_revision: creation.revision_id }
      const discard = await call("discard_fork", discardArgs, `cycle-discard-${index}`)
      assert.deepEqual(await call("discard_fork", discardArgs, `cycle-discard-${index}`), discard)
      assert.deepEqual(await call("create_fork", args, `cycle-create-${index}`), creation)
      if(index===0) {
        const bindingPath = path.join(residentRoot,"resources",resource.split(":")[1],"binding.json")
        const originalBinding = fs.readFileSync(bindingPath)
        try {
          fs.writeFileSync(bindingPath,JSON.stringify({...JSON.parse(originalBinding),version:999}))
          const broken = await fetch(`${baseUrl}/v1/op/session_history`,{method:"POST",headers:{"content-type":"application/json","x-agent-spreadsheet-request-id":"broken-binding-query"},body:JSON.stringify({action:"outcome",resource_id:resource,request_id:`cycle-discard-${index}`})})
          assert.equal(broken.status,503)
          assert.equal((await broken.json()).error.code,"RECOVERY_REQUIRED")
        } finally { fs.writeFileSync(bindingPath,originalBinding) }
      }
      const outcome = await call("session_history", { action: "outcome", resource_id: resource, request_id: `cycle-discard-${index}` }, `cycle-query-${index}`)
      assert.deepEqual(outcome.data.response, discard)
      assert.equal(outcome.revision_id, undefined)
      const diagnostics = await hostRequest(residentRoot, { control: "diagnostics", resource_id: resource }, 503)
      assert.match(JSON.stringify(diagnostics), /owner not attached/)
    }
  })

  await t.test("a stale revision maps 409 onto CanonicalOperationError", async () => {
    const fork = await workbook.createFork()
    await assert.rejects(
      fork.write({
        expected_revision: "0".repeat(64),
        mode: "apply",
        ops: [{
          kind: "set_cells",
          sheet_name: "Sheet1",
          cells: { A1: { kind: "value", value: "stale" } }
        }]
      }),
      (error) => {
        assert.ok(error instanceof CanonicalOperationError, `${error}`)
        assert.equal(error.code, "REVISION_CONFLICT")
        assert.equal(error.details.status, 409)
        assert.equal(error.details.status, error.canonicalStatus)
        return true
      }
    )
    await fork[Symbol.asyncDispose]()
  })

  await t.test("unknown file and native resources map 404 onto CanonicalOperationError", async () => {
    for (const resource of ["wb:wb-does-not-exist", "fork:missing-native-owner"]) await assert.rejects(
      client.workbook(resource).describe(),
      (error) => {
        assert.ok(error instanceof CanonicalOperationError, `${error}`)
        assert.equal(error.code, "RESOURCE_NOT_FOUND")
        assert.equal(error.details.status, 404)
        return true
      }
    )
  })

  await t.test("native VBA snapshots preserve module source and cursor identity", async () => {
    assert.equal(capabilities.includes("inspect_vba"), true)
    const listed = await client.listWorkbooks({})
    const source = client.workbook(listed.data.workbooks.find(item=>item.metadata.slug==="macro").resource_id)
    const fork = await source.createFork()
    const inspect = args => client.canonical.execute("inspect_vba",{resource_id:fork.resourceId,...args})
    const summary = await inspect({view:"project_summary",limit_modules:1})
    assert.equal(summary.data.has_vba,true)
    const name = summary.data.modules[0].name
    const first = await inspect({view:"module_source",module_name:name,limit_lines:1})
    assert.ok(first.data.source.length>0)
    assert.ok(first.data.next_cursor,"fixture must exercise multi-page source")
    const second = await inspect({view:"module_source",module_name:name,limit_lines:1,cursor:first.data.next_cursor})
    assert.equal(second.data.start_line,first.data.start_line+first.data.returned_lines)
    assert.equal(second.revision_id,first.revision_id)
    const file = await client.canonical.execute("inspect_vba",{resource_id:source.resourceId,view:"module_source",module_name:name,limit_lines:2})
    assert.equal(first.data.source+second.data.source,file.data.source)
    await fork.discard()
  })
})
