import { describe, expect, test } from "bun:test"

import { overlayPopoverTop, popoverLeft, popoverTop } from "./popover"

describe("popoverTop", () => {
  test("places below when the popover fits", () => {
    expect(popoverTop(4, 1, 20, 10)).toBe(5)
  })

  test("flips above when space below is insufficient", () => {
    expect(popoverTop(15, 1, 20, 10)).toBe(5)
  })

  test("clamps to the top when neither side fully fits", () => {
    expect(popoverTop(3, 1, 8, 6)).toBe(0)
  })
})

describe("popoverLeft", () => {
  test("right aligns to the anchor edge", () => {
    expect(popoverLeft(80, 15, 120, 30, "right")).toBe(65)
  })

  test("clamps inside the left edge", () => {
    expect(popoverLeft(5, 10, 120, 30, "right")).toBe(0)
  })

  test("clamps inside the right edge", () => {
    expect(popoverLeft(110, 15, 120, 30, "left")).toBe(90)
  })
})

describe("overlayPopoverTop", () => {
  test("overlays the anchor when it fits below", () => {
    expect(overlayPopoverTop(4, 1, 20, 10)).toBe(4)
  })

  test("extends upward while keeping its bottom on the anchor row", () => {
    expect(overlayPopoverTop(15, 1, 20, 10)).toBe(6)
  })
})
