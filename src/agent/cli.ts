#!/usr/bin/env bun

export {}

const DEFAULT_URL = "ws://127.0.0.1:47831/agent"

const [command, argument] = process.argv.slice(2)
if (!command || !["screenshot", "state", "actions", "action"].includes(command)) usage()
if (command === "action" && !argument) usage()

let action: unknown
if (command === "action") {
  try {
    action = JSON.parse(argument!)
  } catch {
    console.error("action must be a valid JSON object")
    process.exit(1)
  }
}

try {
  const result = await request(process.env.MONEY_TUI_AGENT_URL ?? DEFAULT_URL, {
    command,
    ...(action === undefined ? {} : { action }),
  })
  if (command === "screenshot") {
    const filename = Reflect.get(result as object, "filename")
    if (typeof filename !== "string") throw new Error("screenshot response did not include a filename")
    console.log(filename)
  } else {
    console.log(JSON.stringify(result, null, 2))
  }
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error))
  process.exit(1)
}

function usage(): never {
  console.error(`Usage:
  bun run agent screenshot
  bun run agent state
  bun run agent actions
  bun run agent action '<json>'`)
  process.exit(1)
}

function request(url: string, input: Record<string, unknown>): Promise<unknown> {
  const id = crypto.randomUUID()
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(url)
    const timeout = setTimeout(() => {
      socket.close()
      reject(new Error(`timed out connecting to ${url}`))
    }, 5_000)

    socket.addEventListener("open", () => {
      socket.send(JSON.stringify({ id, ...input }))
    })
    socket.addEventListener("message", (event) => {
      const response: unknown = JSON.parse(String(event.data))
      if (!response || typeof response !== "object") return
      const message = response as { id?: unknown; ok?: unknown; result?: unknown; error?: unknown }
      if (message.id !== id) return

      clearTimeout(timeout)
      socket.close()
      if (message.ok === true) {
        resolve(message.result)
      } else {
        reject(new Error(typeof message.error === "string" ? message.error : "agent request failed"))
      }
    })
    socket.addEventListener("error", () => {
      clearTimeout(timeout)
      reject(new Error(`could not connect to ${url}; is money-tui running?`))
    })
  })
}
