import { RegistryContext, useAtomValue } from "@effect-atom/atom-react"
import type { ScrollBoxRenderable } from "@opentui/core"
import { useKeyboard } from "@opentui/react"
import { useContext, useEffect, useRef, useState } from "react"

import { moveOrderedFocus } from "./focus-order"
import { FormButton } from "./form"
import { createDebugCategories, stateAtom, syncLogAtom } from "./store"
import { gray } from "./theme"

type Field = "categories" | "sync"

const fields: Field[] = ["categories", "sync"]

export function DebugSync({ onSync }: { onSync: () => void }) {
  const registry = useContext(RegistryContext)
  const state = useAtomValue(stateAtom)
  const syncLog = useAtomValue(syncLogAtom)
  const logRef = useRef<ScrollBoxRenderable>(null)
  const [field, setField] = useState<Field>("categories")

  const runSync = onSync

  useEffect(() => {
    logRef.current?.scrollTo(Math.max(0, syncLog.lines.length - 1))
  }, [syncLog.lines.length])

  useKeyboard((key) => {
    const tab = key.name === "tab" || key.name === "shift-tab"
    if (tab || key.name === "up" || key.name === "down") {
      key.preventDefault()
      key.stopPropagation()
      const direction = key.shift || key.name === "shift-tab" || key.name === "up" ? -1 : 1
      setField((current) => moveOrderedFocus(fields, current, direction))
      return
    }
    if (state.busy) return
    if (field === "categories" && (key.name === "return" || key.name === "enter")) {
      createDebugCategories(registry)
      return
    }
    if (field === "sync" && (key.name === "return" || key.name === "enter")) {
      runSync()
    }
  })

  return (
    <box style={{ width: "100%", height: "100%", flexDirection: "column", padding: 1 }}>
      <text fg={gray[100]}><strong>Debug Sync</strong></text>
      <text fg={gray[500]}>Runs the real import pipeline and records each payee decision.</text>

      <box style={{ height: 1 }} />
      <box style={{ width: "60%", maxWidth: 56, flexDirection: "column", gap: 1 }}>
        <FormButton
          id="debug-sync-create-categories"
          label="Add income & expense accounts"
          focused={field === "categories"}
          disabled={state.busy}
          onFocus={() => setField("categories")}
          onPress={() => createDebugCategories(registry)}
        />
        <FormButton
          id="debug-sync-run"
          label={state.busy || syncLog.running ? "Syncing" : "Run sync"}
          focused={field === "sync"}
          disabled={state.busy || syncLog.running}
          onFocus={() => setField("sync")}
          onPress={runSync}
        />
      </box>

      <box style={{ height: 1 }} />
      <box style={{ flexDirection: "row", justifyContent: "space-between" }}>
        <text fg={gray[100]}><strong>Pipeline Log</strong></text>
        <text fg={syncLog.running ? gray[300] : gray[500]}>
          {syncLog.running ? "running" : "idle"}
        </text>
      </box>
      <box
        style={{
          flexGrow: 1,
          minHeight: 0,
          flexDirection: "column",
          border: true,
          borderColor: gray[700],
          paddingLeft: 1,
          paddingRight: 1,
        }}
      >
        <scrollbox
          ref={logRef}
          scrollX={false}
          scrollY
          style={{
            width: "100%",
            height: "100%",
            verticalScrollbarOptions: { visible: true },
          }}
        >
          {syncLog.lines.length === 0 ? (
            <text fg={gray[600]}>No sync has run in this session.</text>
          ) : syncLog.lines.map((line, index) => (
            <text key={index} fg={line.startsWith("Sync failed") ? gray[300] : gray[400]} wrapMode="word">
              {line}
            </text>
          ))}
        </scrollbox>
      </box>
    </box>
  )
}
