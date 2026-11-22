#!/usr/bin/env node
/**
 * Builds a clean OpenAI / Codex skills-only plugin ZIP from plugins/formualizer/
 * (omitting local MCP files, which require partner allowlisting on OpenAI's public directory,
 * while preserving extensions.com.openai.requires_local_executor = true).
 */
import { cp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { execFileSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const pluginRoot = resolve(repoRoot, "plugins/formualizer");
const manifest = JSON.parse(await readFile(resolve(pluginRoot, "plugin.json"), "utf8"));
const version = manifest.version;

const outPath = resolve(
  process.argv[2] || resolve(repoRoot, `formualizer-openai-skills-${version}.zip`)
);

const stageDir = resolve(tmpdir(), `formualizer-openai-skills-${Date.now()}`);
const bundleDir = resolve(stageDir, "formualizer");
await mkdir(bundleDir, { recursive: true });

try {
  const openaiManifest = { ...manifest };
  delete openaiManifest.mcp;
  openaiManifest.extensions = {
    "com.openai": {
      requires_local_executor: true
    }
  };
  await writeFile(resolve(bundleDir, "plugin.json"), JSON.stringify(openaiManifest, null, 2) + "\n");
  await cp(resolve(pluginRoot, "README.md"), resolve(bundleDir, "README.md"));
  await cp(resolve(pluginRoot, "LICENSE"), resolve(bundleDir, "LICENSE"));
  await cp(resolve(pluginRoot, "skills"), resolve(bundleDir, "skills"), {
    recursive: true,
    filter: src => !src.includes("/test")
  });

  await mkdir(dirname(outPath), { recursive: true });
  await rm(outPath, { force: true });
  execFileSync("zip", ["-rq", outPath, "."], { cwd: bundleDir });
  console.log(`Built OpenAI skills-only plugin ZIP: ${outPath}`);
} finally {
  await rm(stageDir, { recursive: true, force: true });
}
