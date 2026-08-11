type Account = {
  id: string
  parent_id: string | null
  name: string
  type: "asset" | "liability" | "income" | "expense" | "equity"
}

type Transaction = {
  id: string
  date: string
  payee: string
  amount: number
  category: string | null
  pending: boolean
  notes: string | null
  raw: Record<string, unknown>
}

const accounts: Account[] = [
  { id: "checking", parent_id: null, name: "Checking", type: "asset" },
  { id: "personal-checking", parent_id: "checking", name: "Personal", type: "asset" },
  { id: "household-checking", parent_id: "checking", name: "Household", type: "asset" },
  { id: "travel-checking", parent_id: "checking", name: "Travel", type: "asset" },
  { id: "savings", parent_id: null, name: "Savings", type: "asset" },
  { id: "income", parent_id: null, name: "Income", type: "income" },
  { id: "salary", parent_id: "income", name: "Salary", type: "income" },
  { id: "expenses", parent_id: null, name: "Expenses", type: "expense" },
  { id: "groceries", parent_id: "expenses", name: "Groceries", type: "expense" },
  { id: "dining", parent_id: "expenses", name: "Dining", type: "expense" },
]

const checkingAccountIds = new Set([
  "personal-checking",
  "household-checking",
  "travel-checking",
])
const runId = Date.now().toString(36)
const generatedByAccount = new Map<string, Transaction[]>()
let sequence = 0

const examples = [
  { payee: "NORTHSTAR COFFEE", category: "Dining", minimum: 350, maximum: 1_400 },
  { payee: "TARGET #1842", category: "Shopping", minimum: 2_500, maximum: 14_000 },
  { payee: "GREENWAY MARKET", category: "Groceries", minimum: 1_800, maximum: 12_000 },
  { payee: "CITY TRANSIT", category: "Transportation", minimum: 250, maximum: 2_500 },
  { payee: "RIVERBEND BOOKS", category: "Shopping", minimum: 900, maximum: 6_000 },
  { payee: "HARBOR ELECTRIC", category: "Utilities", minimum: 6_000, maximum: 18_000 },
]

const randomInt = (minimum: number, maximum: number) =>
  Math.floor(Math.random() * (maximum - minimum + 1)) + minimum

const generateTransactions = (accountId: string) => {
  const generated = generatedByAccount.get(accountId) ?? []
  generatedByAccount.set(accountId, generated)
  const count = randomInt(3, 5)
  for (let index = 0; index < count; index++) {
    const example = examples[randomInt(0, examples.length - 1)]
    sequence++
    generated.push({
      id: `demo-${runId}-${String(sequence).padStart(6, "0")}`,
      date: new Date().toISOString().slice(0, 10),
      payee: example.payee,
      amount: -randomInt(example.minimum, example.maximum),
      category: example.category,
      pending: false,
      notes: null,
      raw: { demo: true, accountId, sequence },
    })
  }
  return generated
}

const plugin = definePlugin({
  name: "Demo transaction import",
  version: "1.0.0",
  payeeFormat: "clean",

  async status() {
    return { ready: true, detail: "Generating demo transactions" }
  },

  async accounts() {
    return { accounts }
  },

  async transactions(request: ImportTransactionsRequest) {
    const accountId = request.accountId
    if (!accounts.some((account) => account.id === accountId)) {
      throw new Error(`Unknown account: ${accountId}`)
    }

    if (!checkingAccountIds.has(accountId)) {
      return { transactions: [], pending_ids: [] }
    }

    return {
      transactions: generateTransactions(accountId),
      pending_ids: [],
    }
  },
})

await servePlugin(plugin)
import { definePlugin, servePlugin, type ImportTransactionsRequest } from "../plugin-sdk/index"
