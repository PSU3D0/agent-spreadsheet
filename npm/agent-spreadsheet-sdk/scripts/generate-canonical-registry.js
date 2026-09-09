#!/usr/bin/env node

const { execFileSync } = require("node:child_process")
const fs = require("node:fs")
const path = require("node:path")

const packageRoot = path.resolve(__dirname, "..")
const outputPath = path.join(packageRoot, "src", "generated", "canonical-registry.json")
const asp = process.env.ASP_BINARY || process.argv[2] || path.resolve(packageRoot, "..", "..", "target", "debug", "asp")

const manifest = JSON.parse(execFileSync(asp, ["registry", "--all"], { encoding: "utf8" }))
manifest.generated_by = "asp registry --all"

fs.mkdirSync(path.dirname(outputPath), { recursive: true })
fs.writeFileSync(outputPath, `${JSON.stringify(manifest, null, 2)}\n`)
console.log(`wrote ${manifest.operations.length} operations to ${outputPath}`)

// Derived WASM projection, checked against native Rust schema builders in tests.
const schemaPath = path.resolve(packageRoot, "../../crates/agent-spreadsheet/schema/canonical-schemas.json")
const schemas = {}
for (const operation of manifest.operations) {
  schemas[`${operation.name}_input_schema`] = operation.input_schema
  schemas[`${operation.name}_output_schema`] = operation.output_schema
}
fs.mkdirSync(path.dirname(schemaPath), { recursive: true })
fs.writeFileSync(schemaPath, `${JSON.stringify(schemas)}\n`)
console.log(`wrote generated WASM schema projection to ${schemaPath}`)
