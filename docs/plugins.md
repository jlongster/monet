# Monet plugins

Monet plugins are user commands supervised by the Monet server. They communicate with JSON-RPC 2.0
messages framed as newline-delimited UTF-8 JSON over stdin and stdout.

## Configuration

Monet reads `~/.config/monet/config.json`, or `MONET_CONFIG_PATH` when set. Inference and transaction
import use separate optional processes with the same command configuration:

```json
{
  "inference": {
    "command": ["bun", "/path/to/inference-plugin.ts"],
    "cwd": "/optional/working/directory",
    "env": ["PROVIDER_API_KEY"],
    "config": {},
    "model": null,
    "initializeTimeoutMs": 5000,
    "requestTimeoutMs": 60000
  },
  "import": {
    "command": ["bun", "/path/to/import-plugin.ts"],
    "cwd": "/optional/working/directory",
    "env": ["IMPORT_API_KEY"],
    "config": {},
    "initializeTimeoutMs": 5000,
    "requestTimeoutMs": 60000
  }
}
```

Commands are argv arrays and are never evaluated by a shell. Monet clears the child environment,
retains basic process variables such as `PATH` and `HOME`, and copies only variables named in `env`.
Opaque `config` is passed to the plugin during initialization. Credential values should remain in
environment variables.

Monet starts commands lazily, keeps them alive between requests, restarts them when configuration
changes, drains logs from stderr, and terminates them on shutdown or protocol failure. Monet does not
include a transaction import plugin; users provide their own.

## Transport

- Each stdin/stdout line contains exactly one compact JSON-RPC message ending with `\n`.
- Messages must not exceed 10 MiB.
- Stdout is reserved for protocol messages; logs belong on stderr.
- Requests use string IDs.
- Unexpected EOF means the plugin exited.
- `$/cancelRequest` is reserved for cancellation.

## Lifecycle

The first request is `initialize`:

```json
{"jsonrpc":"2.0","id":"1","method":"initialize","params":{"protocolVersions":[1],"host":{"name":"monet","version":"0.1.0"},"config":{}}}
```

The plugin selects protocol version 1 and reports its identity and capabilities:

```json
{"jsonrpc":"2.0","id":"1","result":{"protocolVersion":1,"plugin":{"name":"Example","version":"1.0.0"},"capabilities":{"inference":false,"transactionImport":true,"structuredOutput":false,"cancellation":false,"payeeFormat":"raw"}}}
```

Monet sends `shutdown` before stopping a healthy process and forcibly terminates it if it does not
exit promptly.

Transaction import plugins may advertise `payeeFormat: "clean"` when their `payee` values are already
display-ready and should bypass Monet payee resolution. SDK import plugins default to `raw` when the
field is omitted. Inference-only plugins omit `payeeFormat`.

For `raw` imports, Monet builds one history-trained payee resolver per sync from the latest explicit
user `set payee` change on each source. Newly inserted transactions are matched before their first
projection rebuild. Accepted names are persisted as a distinct `payee_match` change, so automatic
matches never become training examples. A later explicit user payee change always overrides an
automatic match, regardless of change order. `clean` imports bypass this stage completely.

## Inference methods

### `inference/status`

```json
{"ready":false,"reason":"No provider is connected","setup":{"message":"Connect a provider"}}
```

### `inference/models`

```json
{"models":[{"id":"provider/model","name":"Model","provider":"Provider","capabilities":{}}]}
```

### `inference/generate`

Request:

```json
{"model":"provider/model","system":"Optional system text","prompt":"Required prompt","outputSchema":null,"timeoutMs":60000}
```

Response:

```json
{"text":"Generated text","structured":null,"model":"provider/model","usage":{}}
```

## Transaction import methods

### `import/status`

Returns the same readiness shape as `inference/status`.

### `import/accounts`

Returns the complete imported account hierarchy:

```json
{"accounts":[{"id":"checking","parent_id":null,"name":"Checking","type":"asset"}]}
```

### `import/transactions`

Monet requests one account at a time. New accounts receive `initial: true`; later imports receive a
derived overlapping `startDate`.

```json
{"accountId":"checking","initial":false,"startDate":"2026-07-12"}
```

The response contains transactions in signed integer minor units. `pending_ids` is the complete
pending set for the account response; omitted previously pending transactions are removed.

```json
{
  "transactions": [{
    "id": "tx-1",
    "date": "2026-07-20",
    "payee": "Coffee",
    "amount": -500,
    "category": "Dining",
    "pending": false,
    "opening_balance": false,
    "notes": null,
    "raw": {}
  }],
  "pending_ids": []
}
```

## Initial transfer review

Whenever an import registers one or more new accounts, Monet performs one transfer-only inference
pass after all account transactions are stored. Candidate pairs must include at least one newly
registered account, but may match against transactions from any existing account. Candidates are
limited to different asset/liability accounts, exact opposite amounts, dates within three days, and
sources that remain unassigned.

The model receives all asset/liability accounts and every candidate pair as compact, properly quoted
CSV. Account and pair aliases replace UUIDs, and integer minor-unit amounts are rendered as signed
decimal values such as `-5.00`. All candidates are assumed to be transfers; the structured response
lists only pair aliases that should **not** be reconciled. Monet validates aliases and enforces
one-to-one source usage before applying accepted pairs.

Monet keeps one accepted source as the canonical balanced transaction and persists the other as its
duplicate. The duplicate retains its import identity but materializes no postings, preventing both
imported sides from doubling activity. Synthetic opening balances bypass inference and are balanced
against Monet's local `Opening Balances` equity account.

Incremental imports do not perform income/expense categorization yet. If no new accounts are added,
the transfer review is skipped.

## JavaScript SDK

`plugin-sdk/index.ts` exports `definePlugin`, `servePlugin`, and protocol-facing TypeScript types:

```ts
import { definePlugin, servePlugin } from "./plugin-sdk/index"

const plugin = definePlugin({
  name: "Example import",
  version: "1.0.0",
  async status(config) {},
  async accounts(config) {},
  async transactions(request, config) {},
})

await servePlugin(plugin)
```

Provider and execution failures should use JSON-RPC server errors with concise messages. The SDK
maps thrown errors to code `-32003`.

Monet records `inference/generate` exchanges as correlated
`.money-tui/logs/inference/<id>_req.txt` and `<id>_res.json` files. Request files contain only labeled
system and user prompts, with JSON prompts pretty-printed and real newlines. Responses and errors are
pretty-printed JSON. Set `MONET_INFERENCE_LOG_DIR` to override the directory.
