import type { ReactNode } from "react"

import { gray, selectionBackground } from "./theme"

type FormFieldProps = {
  id?: string
  label: string
  focused: boolean
  zIndex?: number
  contentHeight?: number
  action?: ReactNode
  children: ReactNode
}

export function FormField({ id, label, focused, zIndex = 0, contentHeight = 1, action, children }: FormFieldProps) {
  const height = contentHeight + 1
  return (
    <box id={id} style={{ width: "100%", height, flexDirection: "row", zIndex }}>
      <box style={{ width: 1, height, flexDirection: "column" }}>
        <text style={{ height: 1 }}> </text>
        <text fg={focused ? gray[200] : gray[700]}>{focused ? "▸" : ""}</text>
      </box>
      <box style={{ flexGrow: 1, minWidth: 0, height, flexDirection: "column" }}>
        <box style={{ width: "100%", height: 1, flexDirection: "row", justifyContent: "space-between" }}>
          <text fg={gray[500]}>{label}</text>
          {action}
        </box>
        {children}
      </box>
    </box>
  )
}

type FormInlineFieldProps = {
  id?: string
  label: string
  focused: boolean
  children: ReactNode
}

export function FormInlineField({ id, label, focused, children }: FormInlineFieldProps) {
  return (
    <box id={id} style={{ width: "100%", height: 1, flexDirection: "row" }}>
      <text fg={focused ? gray[200] : gray[700]} style={{ width: 1 }}>
        {focused ? "▸" : ""}
      </text>
      <text fg={gray[500]}>{label}: </text>
      <box style={{ flexGrow: 1, minWidth: 0, height: 1 }}>{children}</box>
    </box>
  )
}

type FormButtonProps = {
  id?: string
  label: string
  focused: boolean
  disabled?: boolean
  onPress: () => void
}

export function FormButton({ id, label, focused, disabled = false, onPress }: FormButtonProps) {
  return (
    <box id={id} style={{ width: "100%", height: 1, flexDirection: "row" }}>
      <text fg={focused && !disabled ? gray[200] : gray[700]} style={{ width: 1 }}>
        {focused && !disabled ? "▸" : ""}
      </text>
      <box
        style={{
          flexGrow: 1,
          minWidth: 0,
          height: 1,
          flexDirection: "row",
          justifyContent: "center",
          backgroundColor: focused && !disabled ? selectionBackground : gray[800],
        }}
        onMouseDown={disabled ? undefined : onPress}
      >
        <text fg={disabled ? gray[600] : focused ? gray[100] : gray[300]}>{label}</text>
      </box>
    </box>
  )
}
