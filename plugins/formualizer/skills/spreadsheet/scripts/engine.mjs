// Local, integrity-checked engine. No package installation or network access.
import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { brotliDecompressSync } from "node:zlib";
import * as bindings from "../engine/formualizer_engine.mjs";

let cached;
const sha256 = bytes => createHash("sha256").update(bytes).digest("hex");
export function loadEngine() {
  if (cached) return cached;
  const provenance = JSON.parse(readFileSync(new URL("../engine/PROVENANCE.json", import.meta.url), "utf8"));
  const compressed = readFileSync(new URL("../engine/formualizer_engine.wasm.br", import.meta.url));
  if (compressed.length !== provenance.compressed.bytes || sha256(compressed) !== provenance.compressed.sha256)
    throw new Error("Bundled engine compressed SHA-256 mismatch");
  const raw = brotliDecompressSync(compressed);
  if (raw.length !== provenance.raw.bytes || sha256(raw) !== provenance.raw.sha256)
    throw new Error("Bundled engine raw SHA-256 mismatch");
  bindings.initSync({ module: raw });
  cached = { runtime: bindings, version: bindings.version(), provenance, integrity: "verified" };
  return cached;
}
