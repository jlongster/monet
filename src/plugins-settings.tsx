import { RegistryContext, useAtomValue } from "@effect-atom/atom-react"
import { useKeyboard } from "@opentui/react"
import { useContext, useEffect, useState } from "react"

import { FilterSelect, type FilterSelectOption } from "./filter-select"
import { moveOrderedFocus } from "./focus-order"
import { FormButton } from "./form"
import {
  inferenceAtom,
  loadImportPlugin,
  loadInference,
  restartImportPlugin,
  restartInference,
  saveInferenceModel,
  testInference,
} from "./store"
import { gray } from "./theme"

type Field = "model" | "restart" | "test" | "importRestart"

const fields: Field[] = ["model", "restart", "test", "importRestart"]
const testPrompt = "Reply with exactly: Monet inference is ready."

export function PluginsSettings() {
  const registry = useContext(RegistryContext)
  const state = useAtomValue(inferenceAtom)
  const [field, setField] = useState<Field>("model")
  const [openSelect, setOpenSelect] = useState(false)
  const [model, setModel] = useState("")

  useEffect(() => {
    loadInference(registry)
    loadImportPlugin(registry)
  }, [registry])

  useEffect(() => {
    if (!state.config) return
    setModel(state.config.model ?? "")
  }, [state.config])

  useKeyboard((key) => {
    if (openSelect) return
    const tab = key.name === "tab" || key.name === "shift-tab"
    if (tab) {
      key.preventDefault()
      key.stopPropagation()
      setField((current) => moveOrderedFocus(fields, current, key.shift || key.name === "shift-tab" ? -1 : 1))
      return
    }
    if (key.name !== "return" && key.name !== "enter") return
    if (field === "test") testInference(registry, testPrompt, model)
    if (field === "restart") restartInference(registry)
    if (field === "importRestart") restartImportPlugin(registry)
  })

  const modelOptions: FilterSelectOption<string>[] = state.models.map((item) => ({
    value: item.id,
    label: item.name,
    detail: item.provider ?? "",
    searchText: `${item.name} ${item.provider ?? ""} ${item.id}`,
    group: item.provider ?? "Other",
  }))
  const pluginDescription = state.status?.plugin
    ? `Using plugin: ${state.status.plugin.name}${state.status.detail ? ` (${state.status.detail})` : ""}`
    : `Using plugin: ${state.error ?? state.message}`
  const importPluginDescription = state.importStatus?.plugin
    ? `Using plugin: ${state.importStatus.plugin.name}${state.importStatus.detail ? ` (${state.importStatus.detail})` : ""}`
    : `Using plugin: ${state.importError ?? state.importMessage}`

  return (
    <box style={{ width: "100%", height: "100%", flexDirection: "column", padding: 1 }}>
      <box
        style={{
          width: "60%",
          maxWidth: 48,
          flexGrow: 1,
          flexDirection: "column",
          gap: 1,
        }}
      >
        <text fg={gray[500]} style={{ marginLeft: 1 }}>{`Config: ${state.configPath ?? "loading"}`}</text>

        <text fg={gray[100]} style={{ marginLeft: 1 }}><strong>Inference</strong></text>

        <box style={{ width: "100%", height: 2, flexDirection: "row", zIndex: openSelect ? 100 : 0 }}>
          <text fg={field === "model" ? gray[200] : gray[700]} style={{ width: 1, height: 2 }}>
            {field === "model" ? "▸" : ""}
          </text>
          <box style={{ flexGrow: 1, minWidth: 0, height: 2, flexDirection: "column" }}>
            <text fg={gray[100]}><strong>Model</strong></text>
            <FilterSelect
              id="inference-model"
              options={modelOptions}
              value={model}
              focused={field === "model"}
              disabled={!state.config || state.models.length === 0 || state.busy}
              placeholder="Filter models"
              onFocus={() => setField("model")}
              onOpenChange={setOpenSelect}
              onSelect={(selected) => {
                setModel(selected)
                saveInferenceModel(registry, selected)
              }}
            />
          </box>
        </box>

        <box style={{ marginLeft: 1, height: 2, flexDirection: "column" }}>
          <text fg={gray[100]}><strong>Plugin</strong></text>
          <text fg={state.error ? gray[300] : state.status?.ready ? gray[300] : gray[500]}>
            {pluginDescription}
          </text>
        </box>

        <FormButton label="Restart plugin" focused={field === "restart"} disabled={state.busy} onPress={() => restartInference(registry)} />
        <box style={{ width: "100%", flexDirection: "column", gap: state.testLog.length > 0 ? 1 : 0 }}>
          <FormButton label="Test generation" focused={field === "test"} disabled={!state.config || state.busy} onPress={() => testInference(registry, testPrompt, model)} />
          {state.testLog.length > 0 ? (
            <box style={{ marginLeft: 1, flexDirection: "column" }}>
              {state.testLog.map((line, index) => (
                <text key={index} fg={line.startsWith("Error:") ? gray[300] : gray[400]} wrapMode="word">
                  {line || " "}
                </text>
              ))}
            </box>
          ) : null}
        </box>

        <box style={{ marginLeft: 1, flexDirection: "column" }}>
          <text fg={gray[100]}><strong>Transaction import</strong></text>
        </box>
        <box style={{ marginLeft: 1, height: 2, flexDirection: "column" }}>
          <text fg={gray[100]}><strong>Plugin</strong></text>
          <text
            fg={state.importError ? gray[300] : state.importStatus?.ready ? gray[300] : gray[500]}
            wrapMode="word"
          >
            {importPluginDescription}
          </text>
        </box>
        <FormButton
          label="Restart plugin"
          focused={field === "importRestart"}
          disabled={state.importBusy}
          onPress={() => restartImportPlugin(registry)}
        />
      </box>
    </box>
  )
}
