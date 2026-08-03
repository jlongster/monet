import { describe, expect, test } from "bun:test"

import { moveOrderedFocus } from "./focus-order"

describe("moveOrderedFocus", () => {
  const fields = ["notes", "counterpart", "split", "register"] as const

  test("moves forward and wraps", () => {
    expect(moveOrderedFocus(fields, "notes", 1)).toBe("counterpart")
    expect(moveOrderedFocus(fields, "register", 1)).toBe("notes")
  })

  test("moves backward and wraps", () => {
    expect(moveOrderedFocus(fields, "counterpart", -1)).toBe("notes")
    expect(moveOrderedFocus(fields, "notes", -1)).toBe("register")
  })
})
