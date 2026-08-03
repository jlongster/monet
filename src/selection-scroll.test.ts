import { describe, expect, test } from "bun:test"

import { scrollTopForRange } from "./selection-scroll"

describe("scrollTopForRange", () => {
  test("does not move while selection remains inside the viewport", () => {
    expect(scrollTopForRange(0, 20, 0)).toBe(0)
    expect(scrollTopForRange(0, 20, 10)).toBe(0)
    expect(scrollTopForRange(0, 20, 19)).toBe(0)
    expect(scrollTopForRange(10, 20, 10)).toBe(10)
    expect(scrollTopForRange(10, 20, 29)).toBe(10)
  })

  test("moves only enough when selection crosses an edge", () => {
    expect(scrollTopForRange(0, 20, 20)).toBe(1)
    expect(scrollTopForRange(10, 20, 9)).toBe(9)
    expect(scrollTopForRange(10, 20, 30)).toBe(11)
  })
})
