#!/usr/bin/env node
"use strict"

// Size gate for the published WASM web bundle.
//
// Builds `--target web` with the `wasm-release` profile and fails when either
// the raw or the brotli-compressed `.wasm` exceeds the ceiling recorded in
// wasm-size-budget.json. Compressed size is what users download; raw size is
// what the engine has to compile and hold in memory.

const fs = require("node:fs")
const path = require("node:path")
const { parseArgs } = require("node:util")

const { build, measure, wasmArtifact } = require("./build-wasm-package.js")

const repositoryRoot = path.resolve(__dirname, "..")
const budgetPath = path.join(repositoryRoot, "wasm-size-budget.json")

function formatBytes(value) {
  return `${value} B (${(value / (1024 * 1024)).toFixed(2)} MiB)`
}

function main() {
  const { values: options } = parseArgs({
    options: {
      "no-build": { type: "boolean" },
      "artifact-dir": { type: "string" }
    }
  })
  const skipBuild = options["no-build"]
  if (options["artifact-dir"] !== undefined && !skipBuild) {
    throw new Error("--artifact-dir requires --no-build")
  }

  const budget = JSON.parse(fs.readFileSync(budgetPath, "utf8"))
  const outDir = options["artifact-dir"] !== undefined
    ? path.resolve(options["artifact-dir"])
    : path.resolve(repositoryRoot, budget.out_dir)

  let measured
  if (skipBuild) {
    measured = measure(wasmArtifact(outDir))
  } else {
    measured = build({
      target: budget.target,
      profile: budget.profile,
      outDir,
      opt: true,
      optLevel: budget.wasm_opt_level
    })
  }

  const checks = [
    ["raw", measured.raw, budget.max_raw_bytes],
    ["brotli", measured.brotli, budget.max_brotli_bytes]
  ]

  let failed = false
  for (const [label, actual, ceiling] of checks) {
    const status = actual <= ceiling ? "ok" : "OVER BUDGET"
    const delta = (((actual - ceiling) / ceiling) * 100).toFixed(1)
    console.log(`${label.padEnd(7)} ${formatBytes(actual)} / ceiling ${formatBytes(ceiling)} — ${status} (${delta}%)`)
    if (actual > ceiling) failed = true
  }

  if (failed) {
    console.error(
      "\nWASM bundle exceeds its size budget.\n" +
      "Either shrink the bundle or raise the ceiling in wasm-size-budget.json with a\n" +
      "reason in the commit message. Ceilings are meant to ratchet down, not up."
    )
    process.exit(1)
  }
  console.log("\nWASM bundle is within budget.")
}

main()
