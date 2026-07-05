# money-tui

A small terminal app for managing personal finances, built with Bun, React, and OpenTUI.

## Development

```sh
bun install
bun run dev
```

Run the type checker with `bun run typecheck`.

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
