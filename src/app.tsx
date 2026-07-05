import type { InputRenderable } from "@opentui/core"
import { useKeyboard, useRenderer } from "@opentui/react"
import { useRef, useState } from "react"

const accounts = [
  { name: "Checking", balance: "$4,280.16" },
  { name: "Savings", balance: "$12,500.00" },
  { name: "Credit card", balance: "-$842.31" },
]

export function App() {
  const renderer = useRenderer()
  const input = useRef<InputRenderable>(null)
  const [submitted, setSubmitted] = useState("Nothing submitted")

  useKeyboard((key) => {
    if (renderer.currentFocusedEditor) return
    if (key.name === "q" || key.name === "escape") {
      renderer.destroy()
    }
  })

  return (
    <box
      style={{
        width: "100%",
        height: "100%",
        flexDirection: "column",
        padding: 1,
        gap: 1,
        backgroundColor: "#101417",
      }}
    >
      <box style={{ flexDirection: "row", justifyContent: "space-between" }}>
        <text fg="#7dd3a7">
          <strong>MONEY</strong>
        </text>
        <text fg="#718078">July 2026</text>
      </box>

      <box
        title=" Net worth "
        style={{ border: true, borderStyle: "rounded", borderColor: "#355247", padding: 1 }}
      >
        <text fg="#f4f1de">
          <strong>$15,937.85</strong>
        </text>
        <text fg="#7dd3a7">+$624.20 this month</text>
      </box>

      <box
        title=" Accounts "
        style={{
          border: true,
          borderStyle: "rounded",
          borderColor: "#2b3934",
          padding: 1,
          gap: 1,
        }}
      >
        {accounts.map((account) => (
          <box key={account.name} style={{ flexDirection: "row", justifyContent: "space-between" }}>
            <text fg="#c9d1cc">{account.name}</text>
            <text fg={account.balance.startsWith("-") ? "#e98578" : "#f4f1de"}>{account.balance}</text>
          </box>
        ))}
      </box>

      <box
        title=" Agent test "
        style={{
          border: true,
          borderStyle: "rounded",
          borderColor: "#355247",
          padding: 1,
          gap: 1,
        }}
      >
        <input
          id="agent-test-input"
          ref={input}
          focused
          placeholder="Type a test message"
          textColor="#f4f1de"
          focusedTextColor="#f4f1de"
          backgroundColor="#18201d"
          focusedBackgroundColor="#1d2924"
          cursorColor="#7dd3a7"
          onSubmit={(value) => setSubmitted(value ? `Submitted: ${value}` : "Submitted an empty value")}
        />
        <box style={{ flexDirection: "row", gap: 1 }}>
          <box
            style={{ width: 13, paddingLeft: 1, paddingRight: 1, backgroundColor: "#29443a" }}
          >
            <text
              id="agent-test-uppercase"
              fg="#b8ead1"
              onMouseDown={() => {
                if (input.current) input.current.value = input.current.value.toUpperCase()
              }}
            >
              Uppercase
            </text>
          </box>
          <box
            style={{ width: 9, paddingLeft: 1, paddingRight: 1, backgroundColor: "#3d302d" }}
          >
            <text
              id="agent-test-clear"
              fg="#f0b5aa"
              onMouseDown={() => {
                if (input.current) input.current.value = ""
                setSubmitted("Cleared")
              }}
            >
              Clear
            </text>
          </box>
        </box>
        <text fg="#718078">{submitted}</text>
      </box>

      <box style={{ flexGrow: 1 }} />
      <text fg="#718078">q / esc quit</text>
    </box>
  )
}
