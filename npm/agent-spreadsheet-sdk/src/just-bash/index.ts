import { defineCommand } from "just-bash"
import { createSessionCommand } from "./session.js"

import { createLocalSpreadsheet, type WasmBindings } from "../local.js"
import {
  adapterEnvelopeFor,
  availableEphemeralOperations,
  executeEphemeralOperation,
  validateAdapterFlags,
  validateEphemeralRequest
} from "./ephemeral.js"
import {
  assertLimit,
  canonicalError,
  descriptorFor,
  discover,
  errorEnvelope,
  jsonResult,
  parseOperationArgs,
  parseOperationParams,
  validateFileBindings
} from "./parser.js"
import { createVfsWriter, readWorkbook, resolveVfsPath } from "./vfs.js"

const DEFAULT_MAX_WORKBOOK_BYTES = 64 * 1024 * 1024
const DEFAULT_MAX_PARAMS_BYTES = 1024 * 1024

/** Options for {@link createAspCommand}. */
export interface AspCommandOptions {
  bindings: WasmBindings
  maxWorkbookBytes?: number
  maxParamsBytes?: number
}

/**
 * Register the `asp` just-bash custom command on the local runtime.
 *
 * Surface: `asp op <operation> [--bind PATH] [--baseline PATH] [--json JSON]
 * [--output PATH|--in-place]`, plus `asp operations`, `asp schema <op>`, and
 * `asp example <op>`. It binds bytes through `ctx.fs` and carries no operation taxonomy.
 */
export type AspCommand = ReturnType<typeof defineCommand> & {
  /** Drain accepted commands and release all resident sessions owned by this command. */
  dispose(): Promise<void>
}

export function createAspCommand(options: AspCommandOptions): AspCommand {
  const { bindings } = options ?? ({} as AspCommandOptions)
  const maxWorkbookBytes = options?.maxWorkbookBytes ?? DEFAULT_MAX_WORKBOOK_BYTES
  const maxParamsBytes = options?.maxParamsBytes ?? DEFAULT_MAX_PARAMS_BYTES
  assertLimit(maxWorkbookBytes, "maxWorkbookBytes")
  assertLimit(maxParamsBytes, "maxParamsBytes")

  const local = createLocalSpreadsheet({ runtime: bindings })
  const { atomicWrite } = createVfsWriter()
  const sessionCommand = createSessionCommand(local, maxWorkbookBytes, maxParamsBytes, atomicWrite)
  let availability: Promise<Set<string>> | undefined

  function available(): Promise<Set<string>> {
    if (!availability) {
      availability = local.capabilities().then((operations) => availableEphemeralOperations("just_bash", {
        operations: new Set(operations),
        canExport: typeof bindings?.exportWorkbook === "function",
        canDispose: typeof bindings?.disposeSession === "function"
      })).catch((error) => {
        availability = undefined
        throw error
      })
    }
    return availability
  }

  const execute = async (args: string[], ctx: any) => {
    let operation: string | undefined
    try {
      if (args[0] === "session") {
        if (typeof bindings.sessionMetadata !== "function" || typeof bindings.disposeSession !== "function") {
          throw canonicalError("CAPABILITY_UNAVAILABLE", "resident commands require sessionMetadata and disposeSession bindings")
        }
        return await sessionCommand.execute(args.slice(1), ctx)
      }
      const availableOperations = await available()
      const discovery = discover(args, availableOperations)
      if (discovery !== null) return jsonResult(discovery)

      const opts = parseOperationArgs(args)
      operation = opts.operation
      const descriptor = descriptorFor(operation!)
      if (!descriptor) throw canonicalError(
        "UNKNOWN_OPERATION", `unknown operation '${operation}'`, operation, "$.operation"
      )
      const plan = descriptor.adapters.just_bash
      if (plan.support_status !== "supported" || !availableOperations.has(operation!)) {
        throw canonicalError(
          "CAPABILITY_UNAVAILABLE",
          `canonical operation '${operation}' is unavailable in the just-bash adapter`,
          operation,
          "adapter"
        )
      }

      const params = parseOperationParams(opts.json, ctx.stdin, maxParamsBytes, operation)
      validateEphemeralRequest(plan, params)
      validateFileBindings(operation!, plan.binding_kind, Boolean(opts.bind), Boolean(opts.baseline))

      const { artifact } = validateAdapterFlags(operation!, plan, params, {
        output: Boolean(opts.output),
        inPlace: Boolean(opts.inPlace),
        outputIsBind: Boolean(opts.output) &&
          resolveVfsPath(ctx, opts.output!) === resolveVfsPath(ctx, opts.bind!)
      })

      const sources = []
      if (opts.bind) sources.push(readWorkbook(ctx, opts.bind, maxWorkbookBytes, "--bind"))
      if (opts.baseline) {
        sources.push(readWorkbook(ctx, opts.baseline, maxWorkbookBytes, "--baseline"))
      }
      const workbooks = await Promise.all(sources)
      const result = await executeEphemeralOperation({
        local,
        operation: operation!,
        params,
        plan,
        workbooks,
        wantsArtifact: artifact && Boolean(opts.output)
      })
      if (result.workbookBytes) {
        await atomicWrite(
          ctx,
          opts.inPlace ? opts.bind : opts.output,
          result.workbookBytes,
          Boolean(opts.inPlace),
          opts.inPlace ? workbooks[0] : undefined
        )
      }
      if (result.artifactBytes && opts.output) {
        await atomicWrite(ctx, opts.output, result.artifactBytes, false)
      }
      return jsonResult(result.response, result.exitCode)
    } catch (error: any) {
      return jsonResult(errorEnvelope(adapterEnvelopeFor(error) ?? error, operation), 1, true)
    }
  }
  let closed = false
  const active = new Set<Promise<Awaited<ReturnType<typeof execute>>>>()
  const command = defineCommand("asp", (args, ctx) => {
    if (closed) return Promise.resolve(jsonResult(canonicalError("CAPABILITY_UNAVAILABLE", "asp command has been disposed"), 1, true))
    const pending = execute(args, ctx)
    active.add(pending)
    void pending.then(() => active.delete(pending), () => active.delete(pending))
    return pending
  })
  return Object.assign(command, { async dispose() {
    closed = true
    await Promise.allSettled([...active])
    await sessionCommand.dispose()
  } })
}

export { DEFAULT_MAX_PARAMS_BYTES, DEFAULT_MAX_WORKBOOK_BYTES }
