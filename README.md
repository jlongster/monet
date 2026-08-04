# money-tui

A small terminal app for managing personal finances, built with Bun, React, and OpenTUI.

## Development

```sh
bun install
bun run dev
```

Run the type checker with `bun run typecheck`.

## Backend

The Rust server owns the SQLite database and transaction projection model. Run it with:

```sh
cargo run --manifest-path server/Cargo.toml
```

It listens on `127.0.0.1:47832` and stores data in `.money-tui/money.db` by default. Override these
with `MONET_SERVER_ADDR` and `MONET_DB_PATH`.

Configure and run a user-hosted importer with:

```sh
curl -X PUT http://127.0.0.1:47832/import \
  -H 'content-type: application/json' \
  --data '{"url":"http://127.0.0.1:9000"}'

curl -X POST http://127.0.0.1:47832/import/run
```

The importer serves `GET /accounts` and `GET /accounts/{account_id}/transactions`. The complete
request and response contract is documented in the architecture note.

The current decisions, operation model, HTTP API, and implementation checklist live in
`/mnt/life/projects/finance-app-architecture.md`.

## Inference

Monet creates its user configuration at `~/.config/monet/config.json`. Override the path with
`MONET_CONFIG_PATH`. Inference plugins are directly executable commands that speak JSON-RPC 2.0 as
newline-delimited JSON over stdin/stdout.

Configure the bundled OpenCode plugin manually:

```json
{
  "inference": {
    "command": ["bun", "inference-plugins/opencode/index.ts"],
    "cwd": "/absolute/path/to/monet",
    "env": [],
    "config": {},
    "model": null,
    "initializeTimeoutMs": 5000,
    "requestTimeoutMs": 60000
  }
}
```

The Inference screen discovers models, tests generation, and restarts the configured process. Command
and plugin-specific settings remain file-owned. See [Inference plugins](docs/inference-plugins.md) for
the protocol and JavaScript SDK.

## Terminal Control

Run Monet in a shared Terminal Control workspace to use it in a real terminal while agents can
inspect and operate the same session:

```sh
termctrl run monet --host opentui --cwd "$PWD" -- bun run dev
```

Reattach later with `termctrl attach monet`. Monet provides a semantic snapshot to Terminal Control
when the host makes that capability available.

See [Architecture Handoff](docs/architecture-handoff.md) for the current implementation state and
the proposed double-entry finance data model.
