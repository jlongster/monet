import { describe, expect, test } from "bun:test"
import { sequentialSpans } from "./typesafe-payee-eval"

describe("sequentialSpans", () => {
  test("preserves hyphens while combining sequential segments", () => {
    expect(sequentialSpans("Chick-fil-a").map((candidate) => candidate.text)).toEqual([
      "Chick",
      "Chick-fil",
      "Chick-fil-a",
      "fil",
      "fil-a",
      "a",
    ])
  })

  test("preserves spaces while combining sequential segments", () => {
    expect(sequentialSpans("Foo bar").map((candidate) => candidate.text)).toEqual([
      "Foo",
      "Foo bar",
      "bar",
    ])
  })

  test("never combines more than three segments", () => {
    const candidates = sequentialSpans("one two three four")
    expect(candidates.some((candidate) => candidate.text === "one two three four")).toBe(false)
    expect(candidates.some((candidate) => candidate.text === "two three four")).toBe(true)
  })
})
