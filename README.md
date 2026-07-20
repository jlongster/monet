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

## Agent control

While the app is running, an agent can inspect and operate its OpenTUI render tree through the
local WebSocket server:

```sh
bun run agent screenshot
bun run agent state
bun run agent actions
bun run agent action '{"type":"pressArrow","direction":"down"}'
```

`screenshot` prints the absolute path of a generated PNG under `.money-tui/screenshots`. `state`
returns the visible text, focus state, interactive elements, and currently available actions.
Actions support `typeText`, `pressKey`, `pressEnter`, `pressArrow`, `focus`, and `click`.

Set `MONEY_TUI_AGENT_PORT` on the app to change the default port (`47831`), and set
`MONEY_TUI_AGENT_URL` for the CLI when connecting to a non-default URL.

See [Agent Control](docs/agent-control.md) for the protocol, action model, configuration, and
implementation details.

See [Architecture Handoff](docs/architecture-handoff.md) for the current implementation state and
the proposed double-entry finance data model.
