import {
  definePlugin,
  servePlugin,
  type GenerateRequest,
} from "../../plugin-sdk/index"

const api = async (method: string, path: string, body?: unknown) => {
  const command = ["opencode2", "api", method, path]
  if (body !== undefined) command.push("--data", JSON.stringify(body))
  const child = Bun.spawn(command, {
    stdin: "ignore",
    stdout: "pipe",
    stderr: "pipe",
  })
  const [stdout, stderr, status] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ])
  if (status !== 0) throw new Error(stderr.trim() || `opencode2 api exited with ${status}`)
  return JSON.parse(stdout)
}

const models = async () => {
  const response = await api("get", "/api/model")
  return response.data as Array<{
    id: string
    modelID: string
    providerID: string
    name: string
    enabled: boolean
    capabilities: unknown
  }>
}

const modelReference = (model: { id: string; providerID: string }) =>
  `${model.providerID}/${model.id}`

const plugin = definePlugin({
  name: "OpenCode",
  version: "1.0.0",
  structuredOutput: true,

  async status() {
    await api("get", "/api/health")
    const available = (await models()).filter((model) => model.enabled)
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
      models: (await models())
        .filter((model) => model.enabled)
        .map((model) => ({
          id: modelReference(model),
          name: model.name,
          provider: model.providerID,
          capabilities: model.capabilities,
        })),
    }
  },

  async generate(request: GenerateRequest) {
    const available = await models()
    const selected = request.model
      ? available.find((model) => modelReference(model) === request.model)
      : undefined
    if (request.model && !selected) throw new Error(`OpenCode model '${request.model}' was not found`)
    const prompt = [
      request.system,
      request.prompt,
      request.outputSchema
        ? `Return only JSON matching this JSON Schema:\n${JSON.stringify(request.outputSchema)}`
        : undefined,
    ].filter(Boolean).join("\n\n")
    const response = await api("post", "/api/generate", {
      prompt,
      ...(selected ? { model: { id: selected.id, providerID: selected.providerID } } : {}),
    })
    const text = response.data.text
    return {
      text,
      ...(request.outputSchema ? { structured: JSON.parse(text) } : {}),
      model: selected ? modelReference(selected) : "default",
    }
  },
})

export default plugin
await servePlugin(plugin)
