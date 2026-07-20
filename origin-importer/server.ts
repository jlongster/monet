import { pathToFileURL } from "node:url"
import { stat } from "node:fs/promises"

type OriginAccount = {
  id: string
  name?: string
  displayName?: string
  fullName?: string
  type?: string
  category?: string
  group?: string
  balance?: number
  balanceUpdatedAt?: string
}

type OriginTransaction = {
  id: string
  account_id: string
  amount: number
  status?: string
  category?: {
    primary?: string
    detailed?: string
    subcategory?: string
  } | null
  date: string
  vendor?: string
  description?: string
  title?: string
  notes?: string
  [key: string]: unknown
}

type Snapshot = {
  accounts: OriginAccount[]
}

type OriginClientLike = {
  fetchAccountDetails(): Promise<{ accounts: OriginAccount[] }>
  fetchTransactions(options?: {
    startDate?: string
    endDate?: string
    accountIds?: string[]
  }): Promise<{ transactions: OriginTransaction[] }>
}

const clientPath = process.env.ORIGIN_CLIENT_PATH ?? "/root/origin-client.js"
const today = new Date().toISOString().slice(0, 10)
const startDate = process.env.ORIGIN_START_DATE ?? `${today.slice(0, 4)}-01-01`
const endDate = process.env.ORIGIN_END_DATE ?? today
let client: OriginClientLike | null = null
let clientModifiedAt = 0
let snapshot: Snapshot | null = null
let refresh: Promise<Snapshot> | null = null

const originClient = async () => {
  const modifiedAt = (await stat(clientPath)).mtimeMs
  if (!client || modifiedAt !== clientModifiedAt) {
    const clientModule = await import(`${pathToFileURL(clientPath).href}?mtime=${modifiedAt}`)
    client = new clientModule.OriginClient()
    clientModifiedAt = modifiedAt
    snapshot = null
    console.log("loaded refreshed private Origin client")
  }
  return client
}

const accountType = (account: OriginAccount): "asset" | "liability" =>
  account.category === "liability" || account.type === "credit" || account.type === "loan"
    ? "liability"
    : "asset"

const title = (value: string | undefined) =>
  (value || "Other")
    .replaceAll("_", " ")
    .replace(/\b\w/g, (letter) => letter.toUpperCase())

const refreshSnapshot = async () => {
  if (refresh) return refresh
  refresh = originClient()
    .then(async (currentClient) => {
      const details = await currentClient.fetchAccountDetails()
      return details
    })
    .then((details) => {
      snapshot = {
        accounts: details.accounts as OriginAccount[],
      }
      console.log(`refreshed Origin accounts: ${snapshot.accounts.length}`)
      return snapshot
    })
    .finally(() => {
      refresh = null
    })
  return refresh
}

const requireSnapshot = () => snapshot ?? refreshSnapshot()

const importedAccounts = (accounts: OriginAccount[]) => {
  const groups = new Map<string, { id: string; name: string; type: "asset" | "liability" }>()
  for (const account of accounts) {
    const type = accountType(account)
    const group = account.group || type
    const key = `${type}:${group}`
    groups.set(key, {
      id: `origin-group:${key}`,
      name: title(group),
      type,
    })
  }
  return [
    ...groups.values().map((group) => ({ ...group, parent_id: null })),
    ...accounts.map((account) => {
      const type = accountType(account)
      const group = account.group || type
      return {
        id: account.id,
        parent_id: `origin-group:${type}:${group}`,
        name: account.fullName || account.displayName || account.name || account.id,
        type,
      }
    }),
  ]
}

const importedTransaction = (transaction: OriginTransaction) => ({
  id: transaction.id,
  date: transaction.date,
  payee: transaction.vendor || transaction.title || transaction.description || null,
  // Origin returns decimal dollars; Monet stores signed integer minor units.
  amount: Math.round(transaction.amount * 100),
  category:
    transaction.category?.subcategory ||
    transaction.category?.detailed ||
    transaction.category?.primary ||
    null,
  pending: transaction.status === "pending",
  opening_balance: false,
  notes: transaction.notes || null,
  raw: transaction,
})

const previousDay = (date: string) => {
  const value = new Date(`${date}T00:00:00.000Z`)
  value.setUTCDate(value.getUTCDate() - 1)
  return value.toISOString().slice(0, 10)
}

const openingBalanceTransaction = (
  account: OriginAccount,
  transactions: ReturnType<typeof importedTransaction>[],
) => {
  if (!Number.isFinite(account.balance)) return null
  const reportedBalance = Math.round(account.balance! * 100)
  const ledgerBalance = accountType(account) === "liability" ? -reportedBalance : reportedBalance
  const activity = transactions.reduce((sum, transaction) => sum + transaction.amount, 0)
  const amount = ledgerBalance - activity
  if (amount === 0) return null
  const firstDate = transactions.map((transaction) => transaction.date).sort()[0]
  const date = firstDate
    ? previousDay(firstDate)
    : account.balanceUpdatedAt?.slice(0, 10) || new Date().toISOString().slice(0, 10)
  return {
    id: `origin-opening:${account.id}`,
    date,
    payee: "Opening balance",
    amount,
    category: "Opening balance",
    pending: false,
    opening_balance: true,
    notes: null,
    raw: {
      origin_opening_balance: true,
      reported_balance: account.balance,
      imported_activity: activity,
    },
  }
}

const json = (value: unknown, status = 200) =>
  Response.json(value, {
    status,
    headers: { "cache-control": "no-store" },
  })

const server = Bun.serve({
  hostname: "127.0.0.1",
  port: Number(process.env.ORIGIN_IMPORTER_PORT ?? 47835),
  async fetch(request) {
    const url = new URL(request.url)
    console.log(`${request.method} ${url.pathname}${url.search}`)
    if (request.method !== "GET") return json({ error: "Method not allowed" }, 405)
    if (url.pathname === "/health") return json({ healthy: true, snapshot: snapshot !== null })

    try {
      if (url.pathname === "/accounts") {
        const current = await refreshSnapshot()
        return json({ accounts: importedAccounts(current.accounts) })
      }

      const match = url.pathname.match(/^\/accounts\/([^/]+)\/transactions$/)
      if (!match) return json({ error: "Not found" }, 404)
      const accountId = decodeURIComponent(match[1])
      const current = await requireSnapshot()
      if (accountId.startsWith("origin-group:")) {
        return json({ transactions: [], pending_ids: [], latest: null })
      }
      const account = current.accounts.find((account) => account.id === accountId)
      if (!account) {
        return json({ error: `Unknown account: ${accountId}` }, 404)
      }
      const requestedStartDate = url.searchParams.get("start_date") || startDate
      const currentClient = await originClient()
      const result = await currentClient.fetchTransactions({
        accountIds: [accountId],
        startDate: requestedStartDate,
        endDate,
      })
      const transactions = result.transactions
      const pending = transactions.filter((transaction) => transaction.status === "pending")
      const posted = transactions.filter((transaction) => transaction.status !== "pending")
      const normalized = transactions.map(importedTransaction)
      const opening = requestedStartDate === startDate
        ? openingBalanceTransaction(account, normalized)
        : null
      return json({
        // Return the full account snapshot on each explicit sync. Stable IDs make this idempotent
        // and ensure edits to older or pending transactions are observed.
        transactions: opening ? [opening, ...normalized] : normalized,
        pending_ids: pending.map((transaction) => transaction.id),
        latest: posted[0]?.id ?? null,
      })
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      console.error(message)
      const status = message.match(/HTTP \d+/)?.[0] ?? "request failed"
      return json(
        { error: `Origin ${status}; regenerate ~/origin-client.js to refresh its credentials` },
        502,
      )
    }
  },
})

console.log(
  `Origin importer listening on ${server.url}; new-account window ${startDate} to ${endDate}`,
)
