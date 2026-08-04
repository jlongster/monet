# Inference plugins

Monet inference plugins are commands supervised by the Monet server. They communicate using
JSON-RPC 2.0 messages framed as newline-delimited UTF-8 JSON over stdin and stdout, following the
same transport rules as MCP and ACP stdio servers.

## Configuration

Configure one inference command through `PUT /inference`:

```json
{
  "command": ["bun", "/path/to/plugin/index.ts"],
  "cwd": "/optional/working/directory",
  "env": ["PROVIDER_API_KEY"],
  "config": {},
  "model": null,
  "initializeTimeoutMs": 5000,
  "requestTimeoutMs": 60000
}
```

The command is an argv array and is never evaluated by a shell. Monet clears the child environment,
retains basic process variables such as `PATH` and `HOME`, and copies only variables named in `env`.
The `config` value is opaque to Monet and is passed to the plugin during initialization. Credential
values should remain in environment variables rather than configuration.

Monet starts the command lazily, keeps it alive between requests, restarts it when configuration
changes, drains logs from stderr, and terminates it on shutdown or protocol failure.

## Transport

- Each stdin/stdout line contains exactly one compact JSON-RPC message.
- Each line ends with `\n` and must not exceed 10 MiB.
- Stdout is reserved for protocol messages; logs belong on stderr.
- Requests use string IDs. Responses may arrive out of order.
- Unexpected EOF means the plugin exited.
- `$/cancelRequest` is reserved for cancellation. Plugins that do not support cancellation advertise
  `cancellation: false`.

## Lifecycle

The first request is `initialize`:

```json
{"jsonrpc":"2.0","id":"1","method":"initialize","params":{"protocolVersions":[1],"host":{"name":"monet","version":"0.1.0"},"config":{}}}
```

The plugin selects protocol version 1 and reports identity and capabilities:

```json
{"jsonrpc":"2.0","id":"1","result":{"protocolVersion":1,"plugin":{"name":"Example","version":"1.0.0"},"capabilities":{"inference":true,"structuredOutput":false,"cancellation":false}}}
```

No inference request is valid before initialization succeeds. Monet sends `shutdown` before stopping
a healthy process and forcibly terminates it if it does not exit promptly.

## Methods

### `inference/status`

Returns readiness without treating missing provider credentials as a protocol failure:

```json
{"ready":false,"reason":"No provider is connected","setup":{"message":"Connect a provider"}}
```

### `inference/models`

Returns opaque model IDs:

```json
{"models":[{"id":"provider/model","name":"Model","provider":"Provider","capabilities":{}}]}
```

### `inference/generate`

Request parameters:

```json
{"model":"provider/model","system":"Optional system text","prompt":"Required prompt","outputSchema":null,"timeoutMs":60000}
```

Response result:

```json
{"text":"Generated text","structured":null,"model":"provider/model","usage":{}}
```

The selected model in Monet configuration is used when the request omits `model`.

## Errors

Plugins use standard JSON-RPC errors where applicable. Provider and plugin execution failures should
use the JSON-RPC server range and include a concise message:

```json
{"jsonrpc":"2.0","id":"4","error":{"code":-32003,"message":"Provider request failed","data":{"retryable":true}}}
```

## JavaScript SDK

`inference-plugin-sdk/index.ts` exports `defineInferencePlugin`, `serveInferencePlugin`, and the
protocol-facing TypeScript types. `serveInferencePlugin` redirects console output to stderr,
implements JSON-RPC dispatch, and reserves stdout for JSONL responses.

The plugin module exports one object:

```ts
import { defineInferencePlugin, serveInferencePlugin } from "./inference-plugin-sdk/index"

const plugin = defineInferencePlugin({
  name: "Example",
  version: "1.0.0",
  async status(config) {},
  async models(config) {},
  async generate(request, config) {},
})

await serveInferencePlugin(plugin)
```

The bundled OpenCode adapter lives at `inference-plugins/opencode/index.ts`. It uses `opencode2 api`,
so OpenCode owns service discovery, authentication, provider connections, and model availability.
