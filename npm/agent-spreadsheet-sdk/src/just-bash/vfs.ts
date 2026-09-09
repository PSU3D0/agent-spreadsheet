type Json = any

// Coordinate SDK writers sharing a VFS, including distinct command registrations.
// This cannot fence writers outside the adapter or promise a durable host fsync.
const WRITE_LOCKS = new WeakMap<object, Map<string, Promise<void>>>()

export function resolveVfsPath(ctx: Json, path: string): string {
  return ctx.fs.resolvePath(ctx.cwd, path)
}

export async function readWorkbook(ctx: Json, path: string, limit: number, flag: string): Promise<Uint8Array> {
  const resolved = resolveVfsPath(ctx, path)
  let stat: Json
  try {
    stat = await ctx.fs.stat(resolved)
  } catch (cause: Json) {
    cause.aspPath = flag
    throw cause
  }
  if (!stat.isFile) throw Object.assign(new Error(`'${path}' is not a file`), { aspPath: flag })
  if (stat.size > limit) {
    throw Object.assign(new Error(`workbook exceeds the ${limit}-byte adapter limit`), {
      aspCode: "INVALID_REQUEST", aspPath: flag
    })
  }
  let bytes: Json
  try {
    bytes = await ctx.fs.readFileBuffer(resolved)
  } catch (cause: Json) {
    cause.aspPath = flag
    throw cause
  }
  if (bytes.byteLength > limit) {
    throw Object.assign(new Error(`workbook exceeds the ${limit}-byte adapter limit`), {
      aspCode: "INVALID_REQUEST", aspPath: flag
    })
  }
  return Uint8Array.from(bytes)
}

export function createVfsWriter(): { atomicWrite: (ctx: Json, target: string, bytes: Uint8Array, replace: boolean, expectedBytes?: Uint8Array) => Promise<void> } {
  let tempSequence = 0

  async function withTargetLock(ctx: Json, target: string, task: () => Promise<void>): Promise<void> {
    const identity = ctx.fsIdentity ?? ctx.fs
    let locks = WRITE_LOCKS.get(identity)
    if (!locks) { locks = new Map(); WRITE_LOCKS.set(identity, locks) }
    const previous = locks.get(target) || Promise.resolve()
    let release!: () => void
    const current = new Promise<void>((resolve) => { release = resolve })
    locks.set(target, current)
    await previous
    try {
      return await task()
    } finally {
      release()
      if (locks.get(target) === current) locks.delete(target)
    }
  }

  async function atomicWrite(ctx: Json, target: string, bytes: Uint8Array, replace: boolean, expectedBytes?: Uint8Array): Promise<void> {
    const resolved = resolveVfsPath(ctx, target)
    return withTargetLock(ctx, resolved, async () => {
      if (!replace && await ctx.fs.exists(resolved)) {
        throw Object.assign(new Error(`output path '${target}' already exists`), {
          aspCode: "INVALID_REQUEST", aspPath: "--output"
        })
      }
      if (expectedBytes) {
        const current = await ctx.fs.readFileBuffer(resolved)
        if (current.length !== expectedBytes.length || !expectedBytes.every((byte, index) => byte === current[index])) {
          throw Object.assign(new Error("source changed before in-place publication"), { aspCode: "REVISION_CONFLICT", aspPath: "--in-place" })
        }
      }
      let temporary: string
      do {
        temporary = `${resolved}.asp-tmp-${++tempSequence}`
      } while (await ctx.fs.exists(temporary))
      try {
        await ctx.fs.writeFile(temporary, bytes)
        await ctx.fs.mv(temporary, resolved)
      } catch (cause: Json) {
        try { await ctx.fs.rm(temporary, { force: true }) } catch { /* best effort */ }
        cause.aspCode = "OPERATION_FAILED"
        cause.aspPath = "adapter_export"
        throw cause
      }
    })
  }

  return { atomicWrite }
}
