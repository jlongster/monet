export type EvalAccount = {
  id: string;
  parentId: string | null;
  name: string;
  type: "income" | "expense";
};

export type EvalPayee = {
  id: string;
  rawPayee: string;
  direction: "inflow" | "outflow" | "zero";
  potentialPayee?: string;
};

export type ReferenceResult = Record<
  string,
  { payee: string; category: string | null }
>;

export type PayeeInput = {
  accounts: EvalAccount[];
  payees: EvalPayee[];
};

export const EVAL_ACCOUNTS: EvalAccount[] = [
  {
    id: "019fd900-0000-7000-8000-000000000001",
    parentId: null,
    name: "Expense",
    type: "expense",
  },
  {
    id: "019fd900-0000-7000-8000-000000000002",
    parentId: "019fd900-0000-7000-8000-000000000001",
    name: "Life",
    type: "expense",
  },
  {
    id: "019fd900-0000-7000-8000-000000000003",
    parentId: "019fd900-0000-7000-8000-000000000001",
    name: "Food",
    type: "expense",
  },
  {
    id: "019fd900-0000-7000-8000-000000000005",
    parentId: "019fd900-0000-7000-8000-000000000001",
    name: "Kids",
    type: "expense",
  },
  {
    id: "019fd900-0000-7000-8000-000000000006",
    parentId: "019fd900-0000-7000-8000-000000000001",
    name: "Clothing",
    type: "expense",
  },
  {
    id: "019fd900-0000-7000-8000-000000000007",
    parentId: null,
    name: "Income",
    type: "income",
  },
  {
    id: "019fd900-0000-7000-8000-000000000008",
    parentId: "019fd900-0000-7000-8000-000000000007",
    name: "Salary",
    type: "income",
  },
  {
    id: "019fd900-0000-7000-8000-000000000009",
    parentId: "019fd900-0000-7000-8000-000000000007",
    name: "Misc",
    type: "income",
  },
];

const selectableAccounts = EVAL_ACCOUNTS.filter(
  (account) => account.parentId !== null,
).toSorted((left, right) => left.name.localeCompare(right.name));

export const normalizePayee = (value: string) =>
  value
    .normalize("NFKC")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, " ")
    .trim();

export const latestDistinctPayees = (
  transactions: Array<{
    accountId: string;
    id: string;
    date: string;
    payee?: string | null;
    amount: number;
  }>,
  limit = 100,
) => {
  const seen = new Set<string>();
  return transactions
    .filter((transaction) => transaction.payee?.trim())
    .toSorted(
      (left, right) =>
        right.date.localeCompare(left.date) ||
        left.accountId.localeCompare(right.accountId) ||
        left.id.localeCompare(right.id),
    )
    .flatMap((transaction) => {
      const rawPayee = transaction.payee!.trim();
      const direction =
        transaction.amount > 0
          ? "inflow"
          : transaction.amount < 0
            ? "outflow"
            : "zero";
      const key = `${normalizePayee(rawPayee)}\0${direction}`;
      if (seen.has(key)) return [];
      seen.add(key);
      return [{ rawPayee, direction }];
    })
    .slice(0, limit)
    .map(({ rawPayee, direction }, index): EvalPayee => ({
      id: `p${index + 1}`,
      rawPayee,
      direction,
    }));
};

export const addPotentialPlaceholders = (payees: EvalPayee[]) => {
  const selected = new Set(
    payees
      .filter((payee) => payee.potentialPayee !== undefined)
      .map((payee) => payee.id),
  );
  for (const payee of payees
    .toSorted((left, right) => stableRank(left.id) - stableRank(right.id))
    .filter((payee) => !selected.has(payee.id))) {
    if (selected.size === 70) break;
    selected.add(payee.id);
  }
  return payees.map((payee) =>
    selected.has(payee.id)
      ? { ...payee, potentialPayee: payee.potentialPayee ?? "EDIT ME" }
      : payee,
  );
};

export const requestHash = async (value: unknown) => {
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(JSON.stringify(value)),
  );
  return Buffer.from(digest).toString("hex");
};

export const combineOpenCodePrompt = (request: {
  system?: string | null;
  prompt: string;
  outputSchema?: unknown;
}) =>
  [
    request.system,
    request.prompt,
    request.outputSchema
      ? `Return only JSON matching this JSON Schema:\n${JSON.stringify(request.outputSchema)}`
      : undefined,
  ]
    .filter(Boolean)
    .join("\n\n");

export const parseModelReference = (reference: string) => {
  const separator = reference.indexOf("/");
  if (separator < 1 || separator === reference.length - 1) {
    throw new Error(`Invalid model reference: ${reference}`);
  }
  return {
    providerID: reference.slice(0, separator),
    id: reference.slice(separator + 1),
  };
};

export const categoryChoices = () =>
  selectableAccounts.map((account, index) => ({
    alias: `c${index + 1}`,
    name: account.name,
  }));

const stableRank = (value: string) => {
  let hash = 2166136261;
  for (const character of value) {
    hash ^= character.charCodeAt(0);
    hash = Math.imul(hash, 16777619);
  }
  return hash >>> 0;
};
