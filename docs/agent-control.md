# Agent Control

Money TUI exposes its live OpenTUI renderer to local agents through a WebSocket server. This lets
an agent inspect the visible screen, discover interactive renderables, execute UI actions, and
capture PNG screenshots while a person watches the same app in their terminal.

## Usage

Start the app normally:

```sh
bun run dev
```

In another shell, use the project CLI:

```sh
bun run agent state
bun run agent actions
bun run agent action '{"type":"pressArrow","direction":"down"}'
bun run agent screenshot
```

`state`, `actions`, and `action` print JSON. `screenshot` prints only the absolute filename of the
generated PNG so an agent can open it directly.

## Commands

### `state`

Returns the current visible character grid, focus state, interactive elements, and available
actions:

```json
{
  "screen": "...",
  "focused": { "renderable": 12, "editor": false },
  "elements": [],
  "actions": []
}
```

### `actions`

Returns only the actions currently available for execution. Actions are derived from the live
render tree, so targets and coordinates should be refreshed after the UI changes.

### `action`

Executes one action through OpenTUI's testing input APIs, requests a render, waits for the renderer
to become idle, and returns the resulting state.

Supported actions:

```json
{ "type": "typeText", "text": "hello" }
{ "type": "pressKey", "key": "s", "modifiers": { "ctrl": true } }
{ "type": "pressEnter" }
{ "type": "pressArrow", "direction": "down" }
{ "type": "focus", "target": 12 }
{ "type": "click", "target": 12, "x": 20, "y": 8 }
```

Supported key modifiers are `ctrl`, `shift`, `meta`, `super`, and `hyper`. Arrow directions are
`up`, `down`, `left`, and `right`.

### `screenshot`

Captures `renderer.currentRenderBuffer.getSpanLines()`, rasterizes the styled cell grid, and writes
a PNG under `.money-tui/screenshots/`. The generated directory is ignored by Git.

The image uses fixed terminal geometry and a bundled native canvas renderer. It represents the
OpenTUI framebuffer rather than taking an operating-system screenshot of the terminal window.

## Action Discovery

`src/agent/actions.ts` walks the current OpenTUI render tree. A visible renderable is exposed when it is
focusable, is the active editor, or has a mouse listener and wins hit testing at its center.

Each exposed element includes:

| Field | Meaning |
| --- | --- |
| `id`, `num` | OpenTUI ID and numeric action target |
| `x`, `y`, `width`, `height` | Screen position and dimensions |
| `focusable`, `focused` | Focus capability and current state |
| `clickable` | Whether the renderable has a reachable mouse listener |
| `editor` | Whether the renderable is the focused editor |

Focused editors expose `typeText` and `pressEnter`. Unfocused focusable elements expose `focus`,
and clickable elements expose a center-point `click`. Up and down arrow actions are always
available.

Use stable, descriptive OpenTUI IDs on interactive components. Numeric targets belong to the
current render tree and should not be persisted across app restarts or major state changes.

## WebSocket Protocol

The server listens on `ws://127.0.0.1:47831/agent` by default. Requests and responses use a small
JSON request/response protocol:

```json
{ "id": "request-id", "command": "actions" }
```

```json
{ "id": "request-id", "ok": true, "result": [] }
```

Action requests include an `action` property. Screenshot results contain
`{ "filename": "/absolute/path.png" }`. Failures return `ok: false` and an `error` string.

The server binds only to localhost. It currently has no authentication and should not be exposed
through a public interface.

## Configuration

| Variable | Purpose |
| --- | --- |
| `MONEY_TUI_AGENT_PORT` | Changes the app server port |
| `MONEY_TUI_AGENT_URL` | Changes the URL used by the CLI |
| `MONEY_TUI_SCREENSHOT_DIR` | Changes the screenshot output directory |

When changing the port, configure both processes:

```sh
MONEY_TUI_AGENT_PORT=48000 bun run dev
MONEY_TUI_AGENT_URL=ws://127.0.0.1:48000/agent bun run agent state
```

## Implementation

| File | Responsibility |
| --- | --- |
| `src/agent/index.ts` | Public entry point for initializing agent control |
| `src/agent/server.ts` | Localhost WebSocket server and command dispatch |
| `src/agent/cli.ts` | Agent-facing command-line client |
| `src/agent/actions.ts` | Element discovery, action generation, and execution |
| `src/agent/screenshot.ts` | OpenTUI framebuffer capture and rasterization |
| `src/index.tsx` | Server and renderer lifecycle integration |

The interaction model follows `packages/simulation/src/frontend/actions.ts` in the OpenCode
repository. Keep the action shapes aligned with that implementation when adding capabilities.

Run the relevant checks after changes:

```sh
bun run test
bun run typecheck
```
