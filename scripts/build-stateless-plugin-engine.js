#!/usr/bin/env node
"use strict";
// Build from the locked dependency graph; never install plugin dependencies.
const fs = require("node:fs");
const path = require("node:path");
const { execFileSync } = require("node:child_process");
const { createHash } = require("node:crypto");
const zlib = require("node:zlib");
const root = path.resolve(__dirname, "..");
const run = (cmd, args) => execFileSync(cmd, args, { cwd: root, encoding: "utf8", maxBuffer: 64 * 1024 * 1024, env: { ...process.env, RUSTC_WRAPPER: "" } }).trim();
const target = process.env.CARGO_TARGET_DIR;
if (!target) throw new Error("Set CARGO_TARGET_DIR to a dedicated build directory.");
const opt = process.env.WASM_OPT || "wasm-opt";
const out = path.join(root, "plugins/formualizer/skills/spreadsheet/engine");
const temporary = path.resolve(target, "plugin-bindgen");
fs.mkdirSync(temporary, { recursive: true });
run("flock", ["-o", "/tmp/fz-size-cargo.lock", "cargo", "build", "--locked", "--config", 'build.rustc-wrapper=""', "--profile", "wasm-release", "--target", "wasm32-unknown-unknown", "-p", "agent-spreadsheet-stateless-wasm"]);
run("wasm-bindgen", [path.resolve(target, "wasm32-unknown-unknown/wasm-release/agent_spreadsheet_stateless_wasm.wasm"), "--target", "web", "--out-name", "formualizer_engine", "--out-dir", temporary]);
const wasm = path.join(temporary, "formualizer_engine_bg.wasm");
run(opt, ["-Oz", "--enable-bulk-memory", "--enable-sign-ext", "--enable-nontrapping-float-to-int", "--enable-mutable-globals", "--enable-reference-types", "--enable-multivalue", "--enable-simd", wasm, "-o", wasm + ".opt"]);
const raw = fs.readFileSync(wasm + ".opt");
const br = zlib.brotliCompressSync(raw, { params: { [zlib.constants.BROTLI_PARAM_QUALITY]: 11 } });
if (br.length >= 5 * 1024 * 1024) throw new Error("Compressed engine reaches the 5 MiB plugin limit");
const hash = bytes => createHash("sha256").update(bytes).digest("hex");
const metadata = JSON.parse(run("cargo", ["metadata", "--locked", "--format-version", "1", "--config", 'build.rustc-wrapper=""']));
const versions = Object.fromEntries(metadata.packages.filter(p => p.name.startsWith("formualizer") || p.name === "agent-spreadsheet-stateless-wasm" || p.name === "agent-spreadsheet").map(p => [p.name, p.version]));
if (versions.formualizer !== "0.14.0") throw new Error("Expected crates.io formualizer 0.14.0");
fs.mkdirSync(out, { recursive: true });
// Keep readable wasm-bindgen glue, omitting the unused network-capable async
// initializer. Both Node and Workers use initSync with local bytes/module.
let glue = fs.readFileSync(path.join(temporary, "formualizer_engine.js"), "utf8");
const loadStart = glue.indexOf("async function __wbg_load(");
const syncStart = glue.indexOf("function initSync(", loadStart);
const asyncStart = glue.indexOf("async function __wbg_init(", syncStart);
if (loadStart < 0 || syncStart < 0 || asyncStart < 0) throw new Error("Unexpected wasm-bindgen glue shape");
glue = glue.slice(0, loadStart) + glue.slice(syncStart, asyncStart) + "export { initSync };\n";
fs.writeFileSync(path.join(out, "formualizer_engine.mjs"), glue);
fs.writeFileSync(path.join(out, "formualizer_engine.wasm.br"), br);
const provenance = {
  source: { repository: "https://github.com/PSU3D0/agent-spreadsheet", commit: run("git", ["rev-parse", "HEAD"]), dirty: Boolean(run("git", ["status", "--porcelain", "--", "Cargo.lock", "Cargo.toml", "crates"])) },
  crates: versions, profile: "wasm-release", optimizer: "-Oz", brotliQuality: 11,
  tools: { rustc: run("rustc", ["--version"]), wasmBindgen: run("wasm-bindgen", ["--version"]), wasmOpt: run(opt, ["--version"]), node: process.version },
  raw: { bytes: raw.length, sha256: hash(raw) },
  compressed: { bytes: br.length, sha256: hash(br) }
};
fs.writeFileSync(path.join(out, "PROVENANCE.json"), JSON.stringify(provenance, null, 2) + "\n");
console.log(JSON.stringify(provenance, null, 2));
