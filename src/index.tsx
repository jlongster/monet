import { createCliRenderer } from "@opentui/core"
import { createRoot } from "@opentui/react"
import { RegistryProvider } from "@effect-atom/atom-react"
import { provideTerminalControl } from "@kitlangton/terminal-control-opentui"

import { App } from "./app"

const renderer = await createCliRenderer()
const terminalControlSemantics = provideTerminalControl(renderer, {
  application: { name: "monet", version: "0.1.0" },
})
renderer.once("destroy", () => terminalControlSemantics.close())
createRoot(renderer).render(
  <RegistryProvider>
    <App />
  </RegistryProvider>,
)
