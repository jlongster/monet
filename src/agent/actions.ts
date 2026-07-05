import type { CliRenderer, Renderable } from "@opentui/core"
import { createMockKeys, createMockMouse } from "@opentui/core/testing"

export type KeyModifiers = {
  ctrl?: boolean
  shift?: boolean
  meta?: boolean
  super?: boolean
  hyper?: boolean
}

export type Action =
  | { type: "typeText"; text: string }
  | { type: "pressKey"; key: string; modifiers?: KeyModifiers }
  | { type: "pressEnter" }
  | { type: "pressArrow"; direction: "up" | "down" | "left" | "right" }
  | { type: "focus"; target: number }
  | { type: "click"; target: number; x: number; y: number }

export type Element = {
  id: string
  num: number
  x: number
  y: number
  width: number
  height: number
  focusable: boolean
  focused: boolean
  clickable: boolean
  editor: boolean
}

export type UIState = {
  screen: string
  focused: {
    renderable?: number
    editor: boolean
  }
  elements: Element[]
  actions: Action[]
}

export type Harness = ReturnType<typeof createHarness>

const decoder = new TextDecoder()

function children(renderable: Renderable) {
  return renderable.getChildren().filter((child): child is Renderable => "num" in child)
}

function all(renderable: Renderable): Renderable[] {
  return [renderable, ...children(renderable).flatMap(all)]
}

function mouseListeners(renderable: Renderable) {
  const general = Reflect.get(renderable, "_mouseListener")
  const specific = Reflect.get(renderable, "_mouseListeners")
  return Boolean(general) || (specific && typeof specific === "object" && Object.keys(specific).length > 0)
}

function hit(renderer: CliRenderer, renderable: Renderable) {
  if (renderable.width <= 0 || renderable.height <= 0) return false
  const x = Math.floor(renderable.screenX + renderable.width / 2)
  const y = Math.floor(renderable.screenY + renderable.height / 2)
  return renderer.hitTest(x, y) === renderable.num
}

export function createHarness(renderer: CliRenderer) {
  return {
    renderer,
    mockInput: createMockKeys(renderer),
    mockMouse: createMockMouse(renderer),
    renderOnce: async () => {
      await new Promise((resolve) => setTimeout(resolve, 0))
      renderer.requestRender()
      await renderer.idle()
      await new Promise((resolve) => setTimeout(resolve, 0))
      renderer.requestRender()
      await renderer.idle()
    },
    screen: () => decoder.decode(renderer.currentRenderBuffer.getRealCharBytes(true)),
  }
}

export function elements(renderer: CliRenderer): Element[] {
  return all(renderer.root)
    .filter((renderable) => renderable.visible && !renderable.isDestroyed)
    .map((renderable) => {
      const clickable = mouseListeners(renderable) && hit(renderer, renderable)
      return {
        id: renderable.id,
        num: renderable.num,
        x: renderable.screenX,
        y: renderable.screenY,
        width: renderable.width,
        height: renderable.height,
        focusable: renderable.focusable,
        focused: renderable.focused,
        clickable,
        editor: renderer.currentFocusedEditor === renderable,
      }
    })
    .filter((element) => element.focusable || element.clickable || element.editor)
}

export function actions(renderer: CliRenderer, options: { text?: string } = {}): Action[] {
  const items = elements(renderer)
  return [
    ...(renderer.currentFocusedEditor
      ? ([{ type: "typeText", text: options.text ?? "hello" }, { type: "pressEnter" }] satisfies Action[])
      : []),
    ...items.filter((item) => item.focusable && !item.focused).map((item) => ({ type: "focus" as const, target: item.num })),
    ...items
      .filter((item) => item.clickable)
      .map((item) => ({
        type: "click" as const,
        target: item.num,
        x: Math.floor(item.x + item.width / 2),
        y: Math.floor(item.y + item.height / 2),
      })),
    { type: "pressArrow", direction: "down" },
    { type: "pressArrow", direction: "up" },
  ]
}

export function state(harness: Harness): UIState {
  return {
    screen: harness.screen(),
    focused: {
      renderable: harness.renderer.currentFocusedRenderable?.num,
      editor: Boolean(harness.renderer.currentFocusedEditor),
    },
    elements: elements(harness.renderer),
    actions: actions(harness.renderer),
  }
}

export async function execute(harness: Harness, action: Action): Promise<UIState> {
  switch (action.type) {
    case "typeText":
      await harness.mockInput.typeText(action.text)
      break
    case "pressKey":
      harness.mockInput.pressKey(action.key, action.modifiers)
      break
    case "pressEnter":
      harness.mockInput.pressEnter()
      break
    case "pressArrow":
      harness.mockInput.pressArrow(action.direction)
      break
    case "focus":
      all(harness.renderer.root).find((item) => item.num === action.target)?.focus()
      break
    case "click":
      await harness.mockMouse.click(action.x, action.y)
      break
  }
  await harness.renderOnce()
  return state(harness)
}

export function parseAction(value: unknown): Action {
  if (!value || typeof value !== "object") throw new Error("action must be a JSON object")
  const action = value as Record<string, unknown>

  switch (action.type) {
    case "typeText":
      if (typeof action.text !== "string") throw new Error("typeText requires text")
      return { type: "typeText", text: action.text }
    case "pressKey":
      if (typeof action.key !== "string") throw new Error("pressKey requires key")
      return { type: "pressKey", key: action.key, modifiers: parseModifiers(action.modifiers) }
    case "pressEnter":
      return { type: "pressEnter" }
    case "pressArrow":
      if (!isDirection(action.direction)) throw new Error("pressArrow requires an up, down, left, or right direction")
      return { type: "pressArrow", direction: action.direction }
    case "focus":
      if (typeof action.target !== "number") throw new Error("focus requires a numeric target")
      return { type: "focus", target: action.target }
    case "click":
      if (typeof action.target !== "number" || typeof action.x !== "number" || typeof action.y !== "number") {
        throw new Error("click requires numeric target, x, and y values")
      }
      return { type: "click", target: action.target, x: action.x, y: action.y }
    default:
      throw new Error(`unsupported action type ${JSON.stringify(action.type)}`)
  }
}

function isDirection(value: unknown): value is "up" | "down" | "left" | "right" {
  return value === "up" || value === "down" || value === "left" || value === "right"
}

function parseModifiers(value: unknown): KeyModifiers | undefined {
  if (value === undefined) return
  if (!value || typeof value !== "object") throw new Error("pressKey modifiers must be an object")
  const input = value as Record<string, unknown>
  const output: KeyModifiers = {}
  for (const key of ["ctrl", "shift", "meta", "super", "hyper"] as const) {
    if (input[key] !== undefined && typeof input[key] !== "boolean") {
      throw new Error(`pressKey modifier ${key} must be a boolean`)
    }
    if (input[key] !== undefined) output[key] = input[key]
  }
  return output
}
