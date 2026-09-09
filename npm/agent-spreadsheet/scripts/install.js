#!/usr/bin/env node

const fs = require("node:fs")
const fsp = require("node:fs/promises")
const path = require("node:path")
const http = require("node:http")
const https = require("node:https")
const { URL } = require("node:url")

const pkg = require("../package.json")

async function main() {
  const triple = supportedTriple(process.platform, process.arch)
  if (!triple) {
    throw new Error(`Unsupported platform: ${process.platform}/${process.arch}`)
  }

  const version = pkg.version
  const asset = `agent-spreadsheet-${triple.asset}`

  const localBinary = process.env.AGENT_SPREADSHEET_LOCAL_BINARY
  const base = process.env.AGENT_SPREADSHEET_DOWNLOAD_BASE_URL ||
    "https://github.com/PSU3D0/agent-spreadsheet/releases/download"

  const url = `${base}/v${version}/${asset}`

  const vendorDir = path.join(__dirname, "..", "vendor")
  await fsp.mkdir(vendorDir, { recursive: true })

  const dest = path.join(vendorDir, triple.dest)
  if (localBinary) {
    await copyLocalBinary(localBinary, dest)
  } else {
    await download(url, dest)
  }

  if (process.platform !== "win32") {
    await fsp.chmod(dest, 0o755)
  }
}

async function copyLocalBinary(source, dest) {
  const sourcePath = path.resolve(source)
  await fsp.copyFile(sourcePath, dest)
}

function supportedTriple(platform, arch) {
  if (platform === "linux" && arch === "x64") {
    return { asset: "linux-x86_64", dest: "agent-spreadsheet" }
  }
  if (platform === "linux" && arch === "arm64") {
    return { asset: "linux-aarch64", dest: "agent-spreadsheet" }
  }
  if (platform === "darwin" && arch === "x64") {
    return { asset: "macos-x86_64", dest: "agent-spreadsheet" }
  }
  if (platform === "darwin" && arch === "arm64") {
    return { asset: "macos-aarch64", dest: "agent-spreadsheet" }
  }
  return null
}

function download(url, dest) {
  return new Promise((resolve, reject) => {
    const file = fs.createWriteStream(dest)
    const client = chooseHttpClient(url)
    const request = client.get(url, (response) => {
      if (response.statusCode && response.statusCode >= 300 && response.statusCode < 400 && response.headers.location) {
        file.close()
        fs.unlink(dest, () => {
          download(response.headers.location, dest).then(resolve, reject)
        })
        return
      }

      if (response.statusCode !== 200) {
        file.close()
        fs.unlink(dest, () => {
          reject(new Error(`Failed to download ${url} (status ${response.statusCode})`))
        })
        return
      }

      response.pipe(file)
      file.on("finish", () => file.close(resolve))
    })

    request.on("error", (error) => {
      file.close()
      fs.unlink(dest, () => reject(error))
    })
  })
}

function chooseHttpClient(rawUrl) {
  const protocol = new URL(rawUrl).protocol
  if (protocol === "http:") {
    return http
  }
  return https
}

main().catch((error) => {
  console.error(`[agent-spreadsheet] install failed: ${error.message}`)
  process.exit(1)
})
