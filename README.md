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

Application and plugin logs append to `.money-tui/monet.log`. Override the path with `MONET_LOG`.
Inference prompts and responses are written to correlated `<id>_req.txt` and `<id>_res.json` files
under `.money-tui/logs/inference`. Request files contain only the system prompt and prompt, with JSON
prompts pretty-printed. Override the directory with `MONET_INFERENCE_LOG_DIR`.

The current decisions, operation model, HTTP API, and implementation checklist live in
`/mnt/life/projects/finance-app-architecture.md`.

## Plugins

Monet creates its user configuration at `~/.config/monet/config.json`. Override the path with
`MONET_CONFIG_PATH`. Inference and transaction import plugins are directly executable commands that
speak JSON-RPC 2.0 as newline-delimited JSON over stdin/stdout.

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
  },
  "import": null
}
```

Monet bundles only the OpenCode inference adapter. Transaction import plugins are user-provided and
configured under `import`. The Plugins screen reports both processes and restarts them; `[s] sync`
runs the configured transaction import. When new accounts are registered, Monet asks the inference
plugin to reject false positives from a system-wide transfer-candidate pass involving those new
accounts. See [Monet plugins](docs/plugins.md) for the protocol and JavaScript SDK.

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
