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
  { id: "savings", parent_id: null, name: "Savings", type: "asset" },
  { id: "income", parent_id: null, name: "Income", type: "income" },
  { id: "salary", parent_id: "income", name: "Salary", type: "income" },
  { id: "expenses", parent_id: null, name: "Expenses", type: "expense" },
  { id: "groceries", parent_id: "expenses", name: "Groceries", type: "expense" },
  { id: "dining", parent_id: "expenses", name: "Dining", type: "expense" },
]

const runId = Date.now().toString(36)
const generated: Transaction[] = []
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

const generateTransactions = () => {
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
      raw: { demo: true, sequence },
    })
  }
}

const json = (value: unknown, status = 200) =>
  Response.json(value, {
    status,
    headers: { "cache-control": "no-store" },
  })

const server = Bun.serve({
  port: Number(process.env.DEMO_IMPORTER_PORT ?? 47833),
  fetch(request) {
    const url = new URL(request.url)
    console.log(`${request.method} ${url.pathname}${url.search}`)

    if (request.method !== "GET") {
      return json({ error: "Method not allowed" }, 405)
    }

    if (url.pathname === "/accounts") {
      return json({ accounts })
    }

    const match = url.pathname.match(/^\/accounts\/([^/]+)\/transactions$/)
    if (!match) {
      return json({ error: "Not found" }, 404)
    }

    const accountId = decodeURIComponent(match[1])
    if (!accounts.some((account) => account.id === accountId)) {
      return json({ error: `Unknown account: ${accountId}` }, 404)
    }

    if (accountId !== "checking") {
      return json({ transactions: [], pending_ids: [], latest: null })
    }

    generateTransactions()
    const after = url.searchParams.get("after")
    const afterIndex = after ? generated.findIndex((transaction) => transaction.id === after) : -1
    const newTransactions = after
      ? generated.slice(afterIndex < 0 ? 0 : afterIndex + 1)
      : generated

    return json({
      transactions: newTransactions,
      pending_ids: [],
      latest: generated.at(-1)?.id ?? null,
    })
  },
})

console.log(`demo importer listening on ${server.url}`)
