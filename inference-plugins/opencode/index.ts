import { definePlugin, servePlugin, type GenerateRequest } from "../../plugin-sdk/index"
import { parseStructured } from "./structured"

const api = async (method: string, path: string, body?: unknown, signal?: AbortSignal) => {
  const command = ["opencode2", "api", method, path]
  if (body !== undefined) command.push("--data", JSON.stringify(body))
  const child = Bun.spawn(command, {
    stdin: "ignore",
    stdout: "pipe",
    stderr: "pipe",
  })
  const cancel = () => child.kill()
  if (signal?.aborted) cancel()
  else signal?.addEventListener("abort", cancel, { once: true })
  const [stdout, stderr, status] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]).finally(() => signal?.removeEventListener("abort", cancel))
  if (signal?.aborted) throw new Error("OpenCode request was cancelled")
  if (status !== 0) throw new Error(stderr.trim() || `opencode2 api exited with ${status}`)
  return JSON.parse(stdout)
}

type OpenCodeModel = {
  id: string
  modelID: string
  providerID: string
  name: string
  enabled: boolean
  capabilities: unknown
}

let modelCache: OpenCodeModel[] | undefined
let defaultModelCache: OpenCodeModel | undefined

const models = async (refresh = false, signal?: AbortSignal) => {
  if (!refresh && modelCache) return modelCache
  const response = await api("get", "/api/model", undefined, signal)
  modelCache = response.data as OpenCodeModel[]
  if (refresh) defaultModelCache = undefined
  return modelCache
}

const defaultModel = async (signal?: AbortSignal) => {
  if (defaultModelCache) return defaultModelCache
  const response = await api("get", "/api/model/default", undefined, signal)
  defaultModelCache = response.data as OpenCodeModel
  return defaultModelCache
}

const modelReference = (model: { id: string; providerID: string }) => `${model.providerID}/${model.id}`

const plugin = definePlugin({
  name: "OpenCode",
  version: "1.0.0",
  structuredOutput: true,
  cancellation: true,

  async status() {
    await api("get", "/api/health")
    const available = (await models(true)).filter((model) => model.enabled)
    return available.length > 0
      ? { ready: true, detail: `${available.length} enabled models` }
      : {
          ready: false,
          reason: "No enabled model is connected",
          setup: { message: "Run opencode2 and use /connect", command: ["opencode2"] },
        }
  },

  async models() {
    return {
      models: (await models(true))
        .filter((model) => model.enabled)
        .map((model) => ({
          id: modelReference(model),
          name: model.name,
          provider: model.providerID,
          capabilities: model.capabilities,
        })),
    }
  },

  async generate(request: GenerateRequest, _config: unknown, signal: AbortSignal) {
    const selected = request.model
      ? (await models(false, signal)).find((model) => modelReference(model) === request.model)
      : await defaultModel(signal)
    if (request.model && !selected) throw new Error(`OpenCode model '${request.model}' was not found`)
    if (!selected) throw new Error("OpenCode default model was not found")
    const prompt = [
      request.system,
      request.prompt,
      request.outputSchema
        ? `Return only JSON matching this JSON Schema:\n${JSON.stringify(request.outputSchema)}`
        : undefined,
    ]
      .filter(Boolean)
      .join("\n\n")
    const response = await api(
      "post",
      "/api/generate",
      {
        prompt,
        model: {
          id: selected.id,
          providerID: selected.providerID,
        },
      },
      signal,
    )
    const text = response.data.text
    return {
      text,
      ...(request.outputSchema ? { structured: parseStructured(text) } : {}),
      model: request.model ? modelReference(selected) : "default",
      usage: response.data.usage,
    }
  },
})

export default plugin
await servePlugin(plugin)
