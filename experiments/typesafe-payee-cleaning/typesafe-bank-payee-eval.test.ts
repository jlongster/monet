import { describe, expect, test } from "bun:test"
import { parseCsv } from "./typesafe-bank-payee-eval"

describe("parseCsv", () => {
  test("reads commas and escaped quotes in quoted fields", () => {
    expect(parseCsv('name,description\r\n1,"Bank, N.A."\r\n2,"Say ""hello"""\r\n')).toEqual([
      ["name", "description"],
      ["1", "Bank, N.A."],
      ["2", 'Say "hello"'],
    ])
  })
})
