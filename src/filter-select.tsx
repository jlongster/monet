import type { BoxRenderable } from "@opentui/core"
import { useKeyboard } from "@opentui/react"
import { useEffect, useRef, useState } from "react"

import { Popover } from "./popover"
import { gray, selectionBackground } from "./theme"

export type FilterSelectOption<Value extends string> = {
  value: Value
  label: string
  detail?: string
  searchText?: string
  group?: string
  depth?: number
}

type FilterSelectProps<Value extends string> = {
  id: string
  options: FilterSelectOption<Value>[]
  value: Value
  focused: boolean
  disabled?: boolean
  placeholder?: string
  onFocus: () => void
  onSelect: (value: Value) => void
  onOpenChange?: (open: boolean) => void
}

export function FilterSelect<Value extends string>({
  id,
  options,
  value,
  focused,
  disabled = false,
  placeholder = "Filter options",
  onFocus,
  onSelect,
  onOpenChange,
}: FilterSelectProps<Value>) {
  const anchorRef = useRef<BoxRenderable>(null)
  const [open, setOpen] = useState(false)
  const [filter, setFilter] = useState("")
  const [highlightedIndex, setHighlightedIndex] = useState(0)
  const normalizedFilter = filter.trim().toLocaleLowerCase()
  const filteredOptions = normalizedFilter
    ? options
        .filter((option) =>
          (option.searchText ?? `${option.label} ${option.detail ?? ""}`)
            .toLocaleLowerCase()
            .includes(normalizedFilter),
        )
        .sort((left, right) =>
          (left.group ?? "").localeCompare(right.group ?? "") ||
          left.label.localeCompare(right.label),
        )
    : options
  const selectedOption = options.find((option) => option.value === value)
  const visibleCount = 10
  const visibleStart = Math.max(
    0,
    Math.min(highlightedIndex - Math.floor(visibleCount / 2), filteredOptions.length - visibleCount),
  )
  const visibleOptions = filteredOptions.slice(visibleStart, visibleStart + visibleCount)

  const setMenuOpen = (next: boolean) => {
    setOpen(next)
    onOpenChange?.(next)
  }

  const openMenu = () => {
    if (disabled) return
    onFocus()
    setFilter("")
    setHighlightedIndex(Math.max(0, options.findIndex((option) => option.value === value)))
    setMenuOpen(true)
  }

  const select = (option: FilterSelectOption<Value>) => {
    onSelect(option.value)
    setMenuOpen(false)
  }

  useEffect(() => {
    setHighlightedIndex((index) => Math.min(index, Math.max(0, filteredOptions.length - 1)))
  }, [filter, filteredOptions.length])

  useEffect(() => {
    if ((!focused || disabled) && open) setMenuOpen(false)
  }, [disabled, focused, open])

  useKeyboard((key) => {
    if (!focused || disabled) return
    if (!open) {
      if (key.name === "return" || key.name === "enter") openMenu()
      return
    }
    if (key.name === "escape") {
      setMenuOpen(false)
      return
    }
    if (key.name === "up" && filteredOptions.length > 0) {
      setHighlightedIndex((index) => Math.max(0, index - 1))
      return
    }
    if (key.name === "down" && filteredOptions.length > 0) {
      setHighlightedIndex((index) => Math.min(filteredOptions.length - 1, index + 1))
      return
    }
    if (key.name === "return" || key.name === "enter") {
      const option = filteredOptions[highlightedIndex]
      if (option) select(option)
    }
  })

  return (
    <box id={`${id}-anchor`} ref={anchorRef} style={{ height: 1, width: "100%", position: "relative" }}>
      {open ? (
        <box
          style={{
             width: "100%",
             height: 1,
             backgroundColor: gray[700],
          }}
        >
          <input
            id={`${id}-filter`}
            focused
            value={filter}
            placeholder={placeholder}
            textColor={gray[100]}
            focusedTextColor={gray[100]}
            backgroundColor={gray[700]}
            focusedBackgroundColor={gray[700]}
            cursorColor={gray[200]}
            style={{ flexGrow: 1, minWidth: 0 }}
            onInput={(value) => {
              setFilter(value)
              setHighlightedIndex(0)
            }}
          />
        </box>
      ) : (
        <box
          id={id}
          style={{
             width: "100%",
             height: 1,
             flexDirection: "row",
            justifyContent: "space-between",
            backgroundColor: focused && !disabled ? selectionBackground : gray[800],
          }}
          onMouseDown={openMenu}
          onMouseOver={disabled ? undefined : onFocus}
        >
          <text fg={disabled ? gray[600] : focused ? gray[100] : gray[300]}>
            {selectedOption?.label ?? "None"}
          </text>
          <text fg={gray[500]}></text>
        </box>
      )}

      <Popover open={open} anchorRef={anchorRef} height={Math.max(1, visibleOptions.length)}>
        <box
          style={{
            width: "100%",
            height: Math.max(1, visibleOptions.length),
            flexDirection: "column",
            backgroundColor: gray[800],
          }}
        >
          {visibleOptions.length === 0 ? (
           <text fg={gray[600]}>No matches</text>
          ) : null}
          {visibleOptions.map((option, visibleIndex) => {
            const index = visibleStart + visibleIndex
            const highlighted = index === highlightedIndex
            return (
              <box
                key={option.value}
                id={`${id}-option-${option.value}`}
                 style={{
                   height: 1,
                   flexDirection: "row",
                  justifyContent: "space-between",
                  backgroundColor: highlighted ? gray[700] : gray[800],
                }}
                onMouseDown={() => select(option)}
              >
                <text fg={highlighted ? gray[100] : gray[300]}>
                  {normalizedFilter ? option.label : `${"  ".repeat(option.depth ?? 0)}${option.label}`}
                </text>
                <text fg={gray[600]}>{option.detail ?? ""}</text>
              </box>
            )
          })}
        </box>
      </Popover>
    </box>
  )
}
