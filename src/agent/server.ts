import type { CliRenderer } from "@opentui/core"

import { actions, createHarness, execute, parseAction, state, type Action, type Harness } from "./actions"
import { captureScreenshot } from "./screenshot"

const DEFAULT_PORT = 47831

type AgentRequest =
  | { id: string | number; command: "screenshot" }
  | { id: string | number; command: "state" }
  | { id: string | number; command: "actions" }
  | { id: string | number; command: "action"; action: Action }

type AgentResponse =
  | { id: string | number; ok: true; result: unknown }
  | { id: string | number | null; ok: false; error: string }

export function startAgentServer(renderer: CliRenderer) {
  const port = Number.parseInt(process.env.MONEY_TUI_AGENT_PORT ?? String(DEFAULT_PORT), 10)
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    throw new Error("MONEY_TUI_AGENT_PORT must be a valid TCP port")
  }

  const harness = createHarness(renderer)
  return Bun.serve({
    hostname: "127.0.0.1",
    port,
    fetch(request, server) {
      if (new URL(request.url).pathname !== "/agent") {
        return new Response("Not found", { status: 404 })
      }
      if (server.upgrade(request)) return
      return new Response("WebSocket upgrade required", { status: 426 })
    },
    websocket: {
      async message(socket, message) {
        let request: AgentRequest
        try {
          request = parseRequest(message)
        } catch (error) {
          send(socket, {
            id: null,
            ok: false,
            error: error instanceof Error ? error.message : String(error),
          })
          return
        }

        try {
          send(socket, { id: request.id, ok: true, result: await handle(harness, request) })
        } catch (error) {
          send(socket, {
            id: request.id,
            ok: false,
            error: error instanceof Error ? error.message : String(error),
          })
        }
      },
    },
  })
}

async function handle(harness: Harness, request: AgentRequest): Promise<unknown> {
  switch (request.command) {
    case "screenshot":
      return { filename: await captureScreenshot(harness.renderer) }
    case "state":
      return state(harness)
    case "actions":
      return actions(harness.renderer)
    case "action":
      return execute(harness, request.action)
  }
}

function parseRequest(message: string | Buffer): AgentRequest {
  const value: unknown = JSON.parse(typeof message === "string" ? message : message.toString("utf8"))
  if (!value || typeof value !== "object") throw new Error("request must be a JSON object")

  const request = value as Partial<AgentRequest>
  if (typeof request.id !== "string" && typeof request.id !== "number") {
    throw new Error("request id must be a string or number")
  }
  switch (request.command) {
    case "screenshot":
    case "state":
    case "actions":
      return { id: request.id, command: request.command }
    case "action":
      return { id: request.id, command: "action", action: parseAction(Reflect.get(request, "action")) }
    default:
      throw new Error("unsupported command")
  }
}

function send(socket: Bun.ServerWebSocket<unknown>, response: AgentResponse) {
  socket.send(JSON.stringify(response))
}
