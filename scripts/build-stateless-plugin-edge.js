#!/usr/bin/env node
"use strict";
// Materialize a deployable Worker outside the plugin tree. Never upload here.
const fs = require("node:fs");
const path = require("node:path");
const { brotliDecompressSync } = require("node:zlib");
const { createHash } = require("node:crypto");
const root = path.resolve(__dirname, "../plugins/formualizer");
if (!process.argv[2]) throw new Error("Usage: node scripts/build-stateless-plugin-edge.js OUTPUT_DIRECTORY");
const out = path.resolve(process.argv[2]);
if (out.startsWith(root + path.sep) || out === root) throw new Error("Edge build must be outside the plugin tree");
const dir = path.join(root, "skills/spreadsheet/engine");
const provenance = JSON.parse(fs.readFileSync(path.join(dir, "PROVENANCE.json")));
const compressed = fs.readFileSync(path.join(dir, "formualizer_engine.wasm.br"));
const hash = b => createHash("sha256").update(b).digest("hex");
if (hash(compressed) !== provenance.compressed.sha256) throw new Error("Compressed hash mismatch");
const raw = brotliDecompressSync(compressed);
if (hash(raw) !== provenance.raw.sha256) throw new Error("Raw hash mismatch");
fs.mkdirSync(out, { recursive: true });
fs.writeFileSync(path.join(out, "formualizer_engine.wasm"), raw);
fs.copyFileSync(path.join(dir, "formualizer_engine.mjs"), path.join(out, "formualizer_engine.mjs"));
let worker = fs.readFileSync(path.join(root, "edge/worker.mjs"), "utf8");
worker = worker.replace("../skills/spreadsheet/engine/formualizer_engine.mjs", "./formualizer_engine.mjs");
const start = worker.indexOf("async function bundledRuntime()");
const end = worker.indexOf("\nconst PROTOCOL_VERSION", start);
worker = worker.slice(0, start) + 'function bundledRuntime() { throw new Error("Supply the build-time module"); }\n' + worker.slice(end);
fs.writeFileSync(path.join(out, "worker.mjs"), worker);
fs.writeFileSync(path.join(out, "entry.mjs"), 'import module from "./formualizer_engine.wasm";\nimport { runtimeFromModule, createEdgeRecalcWorker } from "./worker.mjs";\nexport default createEdgeRecalcWorker({ getRuntime: () => runtimeFromModule(module) });\n');
console.log(out);
