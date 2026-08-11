import { describe, expect, test } from "bun:test"
import { parseStructured } from "./structured"

describe("parseStructured", () => {
  test("parses raw JSON", () => {
    expect(parseStructured('{"value":1}')).toEqual({ value: 1 })
  })

  test("parses one whole-response JSON fence", () => {
    expect(parseStructured('```json\n{"value":1}\n```')).toEqual({ value: 1 })
  })

  test("rejects surrounding prose", () => {
    expect(() => parseStructured('Result:\n```json\n{"value":1}\n```')).toThrow()
  })
})
