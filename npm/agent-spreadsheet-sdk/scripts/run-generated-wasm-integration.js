#!/usr/bin/env node
// Repository-only harness: build the wasm-bindgen Node package with wasm-pack and run
// the integration tests that need a real WASM runtime. Pass --rebuild to force a fresh
// build; otherwise an existing target/sdk-wasm-node is reused.

const { spawnSync } = require("node:child_process")
const fs = require("node:fs")
const path = require("node:path")

const packageRoot = path.resolve(__dirname, "..")
const repositoryRoot = path.resolve(packageRoot, "..", "..")
const outputDirectory = process.env.AGENT_SPREADSHEET_WASM_PACKAGE ||
  path.join(repositoryRoot, "target", "sdk-wasm-node")
const rebuild = process.argv.includes("--rebuild")

// Default to the profile the package actually ships. `dev` is available for a
// fast local loop; `wasm-opt` is skipped here because the size gate owns it and
// it costs minutes without adding behavioural coverage.
const profile = process.env.AGENT_SPREADSHEET_WASM_PROFILE || "wasm-release"
const optimize = process.env.AGENT_SPREADSHEET_WASM_OPT === "1"

if (rebuild || !fs.existsSync(path.join(outputDirectory, "package.json"))) {
  fs.rmSync(outputDirectory, { recursive: true, force: true })
  const build = spawnSync(process.execPath, [
    path.join(repositoryRoot, "scripts", "build-wasm-package.js"),
    "--target", "nodejs",
    "--profile", profile,
    "--out-dir", outputDirectory,
    ...(optimize ? [] : ["--no-opt"])
  ], {
    cwd: repositoryRoot,
    encoding: "utf8",
    stdio: "inherit"
  })
  if (build.error) throw build.error
  if (build.status !== 0) process.exit(build.status || 1)
} else {
  console.log(`reusing ${outputDirectory} (pass --rebuild to rebuild)`)
}

const test = spawnSync(process.execPath, [
  "--test",
  path.join(packageRoot, "test", "local-runtime.integration.test.js"),
  path.join(packageRoot, "test", "just-bash-integration.test.js"),
  path.join(packageRoot, "test", "resident-just-bash.integration.js"),
  path.join(packageRoot, "test", "wasm-goldens.integration.test.js")
], {
  cwd: packageRoot,
  env: { ...process.env, AGENT_SPREADSHEET_WASM_PACKAGE: outputDirectory },
  encoding: "utf8",
  stdio: "inherit"
})
if (test.error) throw test.error
process.exit(test.status || 0)
