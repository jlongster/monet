import { createCliRenderer } from "@opentui/core"
import { createRoot } from "@opentui/react"

import { startAgentServer } from "./agent"
import { App } from "./app"

const renderer = await createCliRenderer()
const agentServer = startAgentServer(renderer)
renderer.once("destroy", () => agentServer.stop())
createRoot(renderer).render(<App />)
