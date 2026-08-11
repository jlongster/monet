import { describe, expect, test } from "bun:test";
import {
  EVAL_ACCOUNTS,
  addPotentialPlaceholders,
  combineOpenCodePrompt,
  latestDistinctPayees,
  parseModelReference,
} from "./core";

const payees = Array.from({ length: 100 }, (_, index) => ({
  id: `p${index + 1}`,
  rawPayee: `Raw Payee ${index + 1}`,
  direction: "outflow" as const,
}));
describe("eval data", () => {
  test("selects latest distinct payees", () => {
    const selected = latestDistinctPayees([
      {
        accountId: "b",
        id: "3",
        date: "2026-01-03",
        payee: "Target",
        amount: -1,
      },
      {
        accountId: "a",
        id: "2",
        date: "2026-01-02",
        payee: "Target",
        amount: -1,
      },
      {
        accountId: "a",
        id: "1",
        date: "2026-01-01",
        payee: "Target",
        amount: 1,
      },
    ]);
    expect(selected.map((item) => [item.rawPayee, item.direction])).toEqual([
      ["Target", "outflow"],
      ["Target", "inflow"],
    ]);
  });

  test("creates 70 editable potential payees", () => {
    expect(
      addPotentialPlaceholders(payees).filter(
        (item) => item.potentialPayee === "EDIT ME",
      ),
    ).toHaveLength(70);
  });

  test("combines the final OpenCode prompt and parses model references", () => {
    expect(
      combineOpenCodePrompt({
        system: "System",
        prompt: "User",
        outputSchema: { type: "object" },
      }),
    ).toBe(
      'System\n\nUser\n\nReturn only JSON matching this JSON Schema:\n{"type":"object"}',
    );
    expect(parseModelReference("provider/model/with/slashes")).toEqual({
      providerID: "provider",
      id: "model/with/slashes",
    });
  });
});
