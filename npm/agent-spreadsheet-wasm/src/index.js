// Loader for the agent-spreadsheet WebAssembly byte/session runtime.
//
// wasm-bindgen's `--target web` glue expects the caller to supply the module.
// In a browser that means fetching a URL; in Node it means reading the `.wasm`
// that ships next to this file. `createWasmRuntime` hides that difference and
// returns the same bindings object either way, consumed by the SDK local client.

import initWasm, * as bindings from "../pkg/agent_spreadsheet_wasm.js"

const WASM_ASSET = "../pkg/agent_spreadsheet_wasm_bg.wasm"

let pending = null

function isNode() {
  return typeof process !== "undefined"
    && process.versions != null
    && process.versions.node != null
}

async function defaultModuleSource() {
  const assetUrl = new URL(WASM_ASSET, import.meta.url)
  if (isNode() && assetUrl.protocol === "file:") {
    const { readFile } = await import("node:fs/promises")
    return readFile(assetUrl)
  }
  // Browsers and Node's fetch-based loaders both handle the URL directly;
  // wasm-bindgen streams it when the server sends application/wasm.
  return assetUrl
}

/**
 * Instantiate the runtime and return its bindings.
 *
 * The module is instantiated at most once per JavaScript module instance; repeated calls resolve
 * to the same bindings object, which also means sessions created through one
 * call are visible to the next.
 *
 * @param {object} [options]
 * @param {string|URL} [options.wasmUrl] explicit location of the `.wasm` asset
 * @param {BufferSource|WebAssembly.Module} [options.wasmBytes] preloaded module
 * @returns {Promise<object>} the wasm-bindgen exports
 */
export async function createWasmRuntime(options = {}) {
  const { wasmUrl, wasmBytes } = options
  if (wasmUrl && wasmBytes) {
    throw new TypeError("pass either wasmUrl or wasmBytes, not both")
  }

  if (!pending) {
    pending = (async () => {
      const moduleOrPath = wasmBytes ?? wasmUrl ?? (await defaultModuleSource())
      await initWasm({ module_or_path: moduleOrPath })
      return bindings
    })().catch((error) => {
      // A failed instantiation must not poison later attempts with different
      // options (a browser caller may retry with explicit bytes).
      pending = null
      throw error
    })
  }
  return pending
}

export default createWasmRuntime
export { bindings as rawBindings }
