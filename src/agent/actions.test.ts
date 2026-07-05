import { expect, test } from "bun:test"

import { BoxRenderable } from "@opentui/core"
import { createTestRenderer } from "@opentui/core/testing"

import { createHarness, elements, execute, parseAction } from "./actions"

test("discovers and executes a clickable renderable", async () => {
  const setup = await createTestRenderer({ width: 20, height: 5 })
  let clicks = 0
  const button = new BoxRenderable(setup.renderer, {
    id: "submit",
    width: 10,
    height: 3,
    onMouseDown: () => clicks++,
  })
  setup.renderer.root.add(button)
  await setup.renderOnce()

  const [element] = elements(setup.renderer)
  expect(element).toMatchObject({ id: "submit", clickable: true })
  await execute(createHarness(setup.renderer), {
    type: "click",
    target: element.num,
    x: Math.floor(element.x + element.width / 2),
    y: Math.floor(element.y + element.height / 2),
  })

  expect(clicks).toBe(1)
  setup.renderer.destroy()
})

test("validates actions received over the socket", () => {
  expect(parseAction({ type: "pressKey", key: "s", modifiers: { ctrl: true } })).toEqual({
    type: "pressKey",
    key: "s",
    modifiers: { ctrl: true },
  })
  expect(() => parseAction({ type: "pressArrow", direction: "forward" })).toThrow()
})
