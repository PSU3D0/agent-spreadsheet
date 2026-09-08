#!/usr/bin/env node
"use strict"

// Build a wasm-bindgen package for the byte/session adapter.
//
// wasm-pack drives cargo and wasm-bindgen; this wrapper adds two things it
// cannot do on its own:
//   * select the workspace `wasm-release` profile (size settings that must not
//     leak into the native `release` profile used for GitHub release binaries),
//   * run a modern `wasm-opt` with the WebAssembly features rustc emits by
//     default (binaryen <= 117 rejects `memory.fill` without --enable-bulk-memory).
//
// Usage:
//   node scripts/build-wasm-package.js --target web --out-dir target/sdk-wasm-web
//   node scripts/build-wasm-package.js --target nodejs --profile dev --no-opt

const { spawnSync } = require("node:child_process")
const fs = require("node:fs")
const os = require("node:os")
const path = require("node:path")
const zlib = require("node:zlib")

const repositoryRoot = path.resolve(__dirname, "..")

// rustc >= 1.82 emits these by default for wasm32-unknown-unknown.
const WASM_FEATURES = [
  "--enable-bulk-memory",
  "--enable-sign-ext",
  "--enable-nontrapping-float-to-int",
  "--enable-mutable-globals",
  "--enable-reference-types",
  "--enable-multivalue",
  // Formualizer 0.9's dependency graph also emits SIMD instructions.
  "--enable-simd"
]

function parseArgs(argv) {
  const options = {
    target: "web",
    profile: "wasm-release",
    outDir: null,
    opt: true,
    optLevel: "-Oz"
  }
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index]
    const next = () => {
      const value = argv[index + 1]
      if (value === undefined) throw new Error(`${arg} requires a value`)
      index += 1
      return value
    }
    if (arg === "--target") options.target = next()
    else if (arg === "--profile") options.profile = next()
    else if (arg === "--out-dir") options.outDir = next()
    else if (arg === "--opt-level") options.optLevel = next()
    else if (arg === "--no-opt") options.opt = false
    else throw new Error(`unknown argument: ${arg}`)
  }
  if (!options.outDir) {
    options.outDir = path.join(repositoryRoot, "target", `sdk-wasm-${options.target}`)
  }
  options.outDir = path.resolve(repositoryRoot, options.outDir)
  return options
}

function findWasmOpt() {
  if (process.env.WASM_OPT) {
    return fs.existsSync(process.env.WASM_OPT) ? process.env.WASM_OPT : null
  }
  const probe = spawnSync("wasm-opt", ["--version"], { encoding: "utf8" })
  if (probe.status === 0) return "wasm-opt"
  // wasm-pack downloads binaryen here when it runs its own optimizer step.
  const cache = path.join(os.homedir(), ".cache", ".wasm-pack")
  if (!fs.existsSync(cache)) return null
  const candidate = fs.readdirSync(cache)
    .filter((entry) => entry.startsWith("wasm-opt-"))
    .map((entry) => path.join(cache, entry, "bin", "wasm-opt"))
    .find((entry) => fs.existsSync(entry))
  return candidate || null
}

function wasmArtifact(outDir) {
  const entries = fs.readdirSync(outDir).filter((entry) => entry.endsWith(".wasm"))
  if (entries.length !== 1) {
    throw new Error(`expected exactly one .wasm in ${outDir}, found ${entries.length}`)
  }
  return path.join(outDir, entries[0])
}

function measure(file) {
  const bytes = fs.readFileSync(file)
  const brotli = zlib.brotliCompressSync(bytes, {
    params: { [zlib.constants.BROTLI_PARAM_QUALITY]: 11 }
  })
  return { raw: bytes.length, brotli: brotli.length }
}

function build(options) {
  fs.rmSync(options.outDir, { recursive: true, force: true })
  const profileArgs = options.profile === "dev"
    ? ["--dev"]
    : options.profile === "release"
      ? ["--release"]
      : ["--profile", options.profile]

  const result = spawnSync("wasm-pack", [
    "build",
    "crates/agent-spreadsheet-wasm",
    "--target", options.target,
    ...profileArgs,
    "--out-dir", options.outDir,
    // This script runs wasm-opt itself so it can pass the required feature flags.
    "--no-opt"
  ], {
    cwd: repositoryRoot,
    stdio: "inherit",
    encoding: "utf8",
    // sccache-style wrappers are configured globally in some environments and
    // are not always usable from the wasm-pack sandbox.
    env: process.env.AGENT_SPREADSHEET_KEEP_RUSTC_WRAPPER
      ? process.env
      : { ...process.env, RUSTC_WRAPPER: "", CARGO_BUILD_RUSTC_WRAPPER: "" }
  })
  if (result.error) throw result.error
  if (result.status !== 0) process.exit(result.status || 1)

  const artifact = wasmArtifact(options.outDir)
  const before = measure(artifact)
  let optimized = false

  if (options.opt) {
    const wasmOpt = findWasmOpt()
    if (wasmOpt) {
      const temporary = `${artifact}.opt`
      const opt = spawnSync(wasmOpt, [
        options.optLevel, ...WASM_FEATURES, artifact, "-o", temporary
      ], { stdio: "inherit", encoding: "utf8" })
      if (opt.error) throw opt.error
      if (opt.status !== 0) process.exit(opt.status || 1)
      fs.renameSync(temporary, artifact)
      optimized = true
    } else {
      console.warn("wasm-opt not found (set WASM_OPT=/path/to/wasm-opt); skipping optimization")
    }
  }

  const after = measure(artifact)
  console.log(JSON.stringify({
    target: options.target,
    profile: options.profile,
    outDir: path.relative(repositoryRoot, options.outDir),
    wasmOpt: optimized ? options.optLevel : null,
    beforeOpt: before,
    raw: after.raw,
    brotli: after.brotli
  }, null, 2))
  return { artifact, ...after }
}

module.exports = { build, measure, parseArgs, wasmArtifact }

if (require.main === module) {
  build(parseArgs(process.argv.slice(2)))
}
