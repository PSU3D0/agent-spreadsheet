import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp,readFile,writeFile,rm } from "node:fs/promises";
import { join,resolve } from "node:path";
import { tmpdir } from "node:os";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { syntheticWorkbook } from "../scripts/synthetic.mjs";
import { inspectFile,loadRuntime } from "../scripts/run.mjs";
const loader=process.env.FORMUALIZER_TEST_RUNTIME;
const real=(name,fn)=>test(name,fn);
const cli=fileURLToPath(new URL("../scripts/run.mjs",import.meta.url));
function invoke(command,input,request) {const prefix=loader?["--dev-runtime",loader]:[];return spawnSync(process.execPath,[cli,...prefix,command,input,request],{encoding:"utf8"});}
async function temporary(t) {const path=await mkdtemp(join(tmpdir(),"form270-"));t.after(()=>rm(path,{recursive:true,force:true}));return path;}
test("read result freed on metadata failure and registry has no implicit inspection fallback",async t=>{
 const dir=await temporary(t),input=join(dir,"input.xlsx");await writeFile(input,syntheticWorkbook());let freed=0;
 await assert.rejects(inspectFile({inspectXlsx:()=>({metadataJson:"bad",free(){freed++;}})},input,"{}"));assert.equal(freed,1);
 await assert.rejects(inspectFile({},input,"{}"),/matching development runtime/);
});
real("native read and one-hop CLI trace keep stored cache and engine value distinct",async t=>{
 const dir=await temporary(t),input=join(dir,"input.xlsx"),request=join(dir,"request.json");
 const original=syntheticWorkbook();await writeFile(input,original);
 await writeFile(request,JSON.stringify({selection:{kind:"cells",addresses:["Sheet1!B1"]}}));
 const inspected=invoke("inspect",input,request);assert.equal(inspected.status,0,inspected.stderr);
 const sourceWire=JSON.parse(inspected.stdout);
 assert.equal(sourceWire.runtime.kind,loader?"development":"registry");
 if (loader) {
   assert.equal(sourceWire.runtime.module,resolve(loader));
   assert.equal(sourceWire.runtime.registry_version,null);
 } else {
   assert.equal(sourceWire.runtime.version,"0.17.0");
 }
 const source=sourceWire.report;
 assert.equal(source.items[0].stored_formula_cache.value.value,42);
 assert.equal(source.items[0].stored_formula_cache.freshness,"unverified");
 await writeFile(request,JSON.stringify({roots:["Sheet1!B1"]}));
 const traced=invoke("trace",input,request);assert.equal(traced.status,0,traced.stderr);
 const traceWire=JSON.parse(traced.stdout);
 assert.equal(traceWire.runtime.kind,loader?"development":"registry");
 if (loader) {
   assert.equal(traceWire.runtime.module,sourceWire.runtime.module);
   assert.equal(traceWire.runtime.registry_version,null);
 } else {
   assert.equal(traceWire.runtime.version,"0.17.0");
 }
 const trace=traceWire.report;
 assert.equal(trace.items[0].cell.value,null);assert.equal(trace.items[0].cell.staleness,"NeverEvaluated");
 assert.equal(trace.items[1].cell.value.value,21);assert.equal(trace.coverage.engine_limits.max_depth,0);
 assert.equal(trace.items[0].links[0].kind.provenance,"Declared");
 assert.deepEqual(await readFile(input),original);
});
real("real read results support paging/rerooting and plain stale/unknown-option errors",async t=>{
 const dir=await temporary(t),input=join(dir,"input.xlsx");const original=syntheticWorkbook();await writeFile(input,original);
 const {runtime}=await loadRuntime(loader);
 let freed=0;const wrapped={inspectXlsx(...args){const result=runtime.inspectXlsx(...args);const free=result.free.bind(result);result.free=()=>{freed++;free();};return result;}};
 const first=await inspectFile(wrapped,input,JSON.stringify({selection:{kind:"range",address:"Sheet1!A1:B2"},page:{limit:1}}));
 const second=await inspectFile(wrapped,input,JSON.stringify(first.next_request));
 assert.notEqual(first.items[0].address,second.items[0].address);assert.equal(freed,2);
 const trace=await inspectFile(runtime,input,JSON.stringify({roots:["Sheet1!B1"]}),true);
 const reroot=await inspectFile(runtime,input,JSON.stringify(trace.items[1].next_trace_request),true);
 assert.equal(reroot.items[0].cell.address,"Sheet1!A1");
 const literalRequest=join(dir,"next-trace.json");
 await writeFile(literalRequest,JSON.stringify(trace.items[1].next_trace_request));
 const freshProcess=invoke("trace",input,literalRequest);
 assert.equal(freshProcess.status,0,freshProcess.stderr);
 assert.equal(JSON.parse(freshProcess.stdout).report.items[0].cell.address,"Sheet1!A1");
 const firstTracePage=await inspectFile(runtime,input,JSON.stringify({roots:["Sheet1!B1"],page:{limit:1}}),true);
 await writeFile(literalRequest,JSON.stringify(firstTracePage.next_request));
 const nextProcess=invoke("trace",input,literalRequest);
 assert.equal(nextProcess.status,0,nextProcess.stderr);
 const secondTracePage=JSON.parse(nextProcess.stdout).report;
 assert.equal(secondTracePage.fingerprint,firstTracePage.fingerprint);
 assert.notEqual(secondTracePage.items[0].cell.address,firstTracePage.items[0].cell.address);
 for(const body of [{roots:["Sheet1!B1"],calculate:true},{roots:["Sheet1!B1"],page:{offset:1}}]) {
   let caught;try{runtime.traceXlsx(new Uint8Array(original),JSON.stringify(body));}catch(error){caught=error;}
   assert.ok(["INVALID_ARGUMENT","STALE_CURSOR"].includes(caught.code));assert.equal(typeof caught.message,"string");assert.ok(caught.message.length);
 }
 const changed={...first.next_request,page:{...first.next_request.page,limit:2}};
 await assert.rejects(inspectFile(runtime,input,JSON.stringify(changed)),error=>error.code==="STALE_CURSOR");
 assert.deepEqual(await readFile(input),original);
});
real("style-free source supports inspect and trace without entering rich export",async t=>{
 const dir=await temporary(t),input=join(dir,"style-free.xlsx");
 const original=syntheticWorkbook({"xl/styles.xml":null,"xl/_rels/workbook.xml.rels":'<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>'});
 await writeFile(input,original);const {runtime}=await loadRuntime(loader);
 const report=await inspectFile(runtime,input,JSON.stringify({selection:{kind:"cells",addresses:["Sheet1!B1"]}}));
 assert.equal(report.items[0].stored_formula_cache.freshness,"unverified");
 const trace=await inspectFile(runtime,input,JSON.stringify({roots:["Sheet1!B1"]}),true);
 assert.equal(trace.items[0].cell.staleness,"NeverEvaluated");
 assert.deepEqual(await readFile(input),original);
});

real("signed-zero markers survive actual inspect and trace CLI JSON while positive zeros stay unmarked",async t=>{
 const dir=await temporary(t),input=join(dir,"signed-zero.xlsx"),request=join(dir,"request.json");
 const original=syntheticWorkbook({"xl/worksheets/sheet1.xml":'<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><v>-0.0</v></c><c r="B1"><f>A1</f><v>-0.0</v></c><c r="C1"><v>0.0</v></c><c r="D1"><f>C1</f><v>0.0</v></c></row></sheetData></worksheet>'});
 await writeFile(input,original);
 await writeFile(request,JSON.stringify({selection:{kind:"range",address:"Sheet1!A1:D1"}}));
 const inspected=invoke("inspect",input,request);assert.equal(inspected.status,0,inspected.stderr);
 const source=JSON.parse(inspected.stdout).report;
 assert.equal(source.items[0].source_value.negative_zero,true);
 assert.equal(source.items[1].stored_formula_cache.value.negative_zero,true);
 assert.equal(source.items[2].source_value.negative_zero,undefined);
 assert.equal(source.items[3].stored_formula_cache.value.negative_zero,undefined);
 assert.equal(typeof source.items[0].source_value.value,"number");
 await writeFile(request,JSON.stringify({roots:["Sheet1!A1","Sheet1!C1"]}));
 const traced=invoke("trace",input,request);assert.equal(traced.status,0,traced.stderr);
 const engine=JSON.parse(traced.stdout).report;
 assert.equal(engine.items[0].cell.value.negative_zero,true);
 assert.equal(engine.items[1].cell.value.negative_zero,undefined);
 assert.equal(typeof engine.items[0].cell.value.value,"number");
 assert.deepEqual(await readFile(input),original);
});
