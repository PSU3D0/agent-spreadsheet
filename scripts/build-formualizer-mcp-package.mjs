#!/usr/bin/env node
/**
 * Syncs the bundled stateless plugin runtime from plugins/formualizer/
 * into npm/formualizer-mcp/dist/ so @formualizer/mcp stays single-sourced
 * with zero duplicated WASM binaries checked into git.
 */
import { cp, mkdir, rm, copyFile, chmod } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const pluginRoot = resolve(repoRoot, "plugins/formualizer");
const pkgRoot = resolve(repoRoot, "npm/formualizer-mcp");
const distRoot = resolve(pkgRoot, "dist");

await rm(distRoot, { recursive: true, force: true });
await mkdir(resolve(distRoot, "server"), { recursive: true });
await mkdir(resolve(distRoot, "skills/spreadsheet/scripts"), { recursive: true });
await mkdir(resolve(distRoot, "skills/spreadsheet/engine"), { recursive: true });

await copyFile(
  resolve(pluginRoot, "server/stdio-mcp.mjs"),
  resolve(distRoot, "server/stdio-mcp.mjs")
);
await copyFile(
  resolve(pluginRoot, "skills/spreadsheet/scripts/xlsx.mjs"),
  resolve(distRoot, "skills/spreadsheet/scripts/xlsx.mjs")
);
await copyFile(
  resolve(pluginRoot, "skills/spreadsheet/scripts/engine.mjs"),
  resolve(distRoot, "skills/spreadsheet/scripts/engine.mjs")
);
await cp(
  resolve(pluginRoot, "skills/spreadsheet/engine"),
  resolve(distRoot, "skills/spreadsheet/engine"),
  { recursive: true }
);
await copyFile(resolve(pluginRoot, "LICENSE"), resolve(pkgRoot, "LICENSE"));
await chmod(resolve(pkgRoot, "bin/formualizer-mcp.mjs"), 0o755);

console.log(`Synced @formualizer/mcp dist from ${pluginRoot}`);
