import { createInterface } from "node:readline"

export type PluginStatus = {
  ready: boolean
  detail?: string
  reason?: string
  setup?: unknown
}

export type InferenceModel = {
  id: string
  name: string
  provider?: string
  capabilities?: unknown
}

export type GenerateRequest = {
  model?: string
  system?: string
  prompt: string
  outputSchema?: unknown
  timeoutMs?: number
}

export type GenerateResponse = {
  text: string
  structured?: unknown
  model: string
  usage?: unknown
}

export type InferencePlugin = {
  name: string
  version: string
  status(config: unknown): Promise<PluginStatus>
  models(config: unknown): Promise<{ models: InferenceModel[] }>
  generate(request: GenerateRequest, config: unknown): Promise<GenerateResponse>
  structuredOutput?: boolean
  shutdown?(): Promise<void>
}

export type ImportedAccount = {
  id: string
  parent_id: string | null
  name: string
  type: "asset" | "liability" | "income" | "expense" | "equity"
}

export type ImportedTransaction = {
  id: string
  date: string
  payee?: string | null
  amount: number
  category?: string | null
  pending?: boolean
  opening_balance?: boolean
  notes?: string | null
  raw?: unknown
}

export type ImportTransactionsRequest = {
  accountId: string
  initial: boolean
  startDate?: string
}

export type ImportPlugin = {
  name: string
  version: string
  payeeFormat?: "clean" | "raw"
  status(config: unknown): Promise<PluginStatus>
  accounts(config: unknown): Promise<{ accounts: ImportedAccount[] }>
  transactions(
    request: ImportTransactionsRequest,
    config: unknown,
  ): Promise<{
    transactions: ImportedTransaction[]
    pending_ids?: string[]
  }>
  shutdown?(): Promise<void>
}

export type MonetPlugin = InferencePlugin | ImportPlugin

export const definePlugin = <Plugin extends MonetPlugin>(plugin: Plugin) => plugin

export const servePlugin = async (plugin: MonetPlugin) => {
  const stderr = (...values: unknown[]) => {
    process.stderr.write(`${values.map(String).join(" ")}\n`)
  }
  console.log = stderr
  console.info = stderr
  console.warn = stderr
  console.debug = stderr

  let initialized = false
  let config: unknown = null
  const send = (message: unknown) => process.stdout.write(`${JSON.stringify(message)}\n`)
  const result = (id: unknown, value: unknown) => send({ jsonrpc: "2.0", id, result: value })
  const error = (id: unknown, code: number, message: string, data?: unknown) =>
    send({ jsonrpc: "2.0", id, error: { code, message, ...(data === undefined ? {} : { data }) } })

  for await (const line of createInterface({ input: process.stdin, crlfDelay: Infinity })) {
    if (!line) continue
    let message: any
    try {
      message = JSON.parse(line)
    } catch (cause) {
      error(null, -32700, "Parse error", String(cause))
      continue
    }
    const id = message.id ?? null
    if (message.jsonrpc !== "2.0" || typeof message.method !== "string") {
      error(id, -32600, "Invalid Request")
      continue
    }
    try {
      if (message.method === "initialize") {
        if (initialized) throw new Error("initialize may only be called once")
        if (!message.params?.protocolVersions?.includes(1)) {
          error(id, -32602, "No supported protocol version")
          continue
        }
        config = message.params.config ?? null
        initialized = true
        const inference = "models" in plugin && "generate" in plugin
        const transactionImport = "accounts" in plugin && "transactions" in plugin
        result(id, {
          protocolVersion: 1,
          plugin: { name: plugin.name, version: plugin.version },
          capabilities: {
            inference,
            transactionImport,
            structuredOutput: inference && plugin.structuredOutput === true,
            cancellation: false,
            ...(transactionImport && "accounts" in plugin
              ? { payeeFormat: plugin.payeeFormat ?? "raw" }
              : {}),
          },
        })
      } else if (!initialized) {
        error(id, -32002, "Plugin is not initialized")
      } else if (message.method === "inference/status" && "generate" in plugin) {
        result(id, await plugin.status(config))
      } else if (message.method === "inference/models" && "models" in plugin) {
        result(id, await plugin.models(config))
      } else if (message.method === "inference/generate" && "generate" in plugin) {
        result(id, await plugin.generate(message.params, config))
      } else if (message.method === "import/status" && "accounts" in plugin) {
        result(id, await plugin.status(config))
      } else if (message.method === "import/accounts" && "accounts" in plugin) {
        result(id, await plugin.accounts(config))
      } else if (message.method === "import/transactions" && "transactions" in plugin) {
        result(id, await plugin.transactions(message.params, config))
      } else if (message.method === "shutdown") {
        await plugin.shutdown?.()
        result(id, null)
        process.stdin.pause()
      } else if (message.method !== "$/cancelRequest") {
        error(id, -32601, "Method not found")
      }
    } catch (cause) {
      error(id, -32003, cause instanceof Error ? cause.message : String(cause))
    }
  }
}
