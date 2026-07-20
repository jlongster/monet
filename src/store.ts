import { Atom, type Registry } from "@effect-atom/atom-react"
import { Effect } from "effect"
import { appendFile, mkdir } from "node:fs/promises"

export type Account = {
  id: string
  parent_id: string | null
  external_id: string | null
  name: string
  type: "asset" | "liability" | "income" | "expense" | "equity"
  latest_external_transaction_id: string | null
  balance: number
  has_children: boolean
}

export type AccountNode = Account & { depth: number }

export type AccountTransaction = {
  source_id: string
  account_id: string
  date: string
  payee: string | null
  amount: number
  pending: boolean
  notes: string | null
  posting_count: number
  unallocated_amount: number
  other_postings: Array<{
    posting_id: string
    account_id: string
    account_name: string
    amount: number
  }>
}

type AccountCursor = {
  date: string
  source_id: string
}

type AccountPage = {
  transactions: AccountTransaction[]
  next: AccountCursor | null
}

type State = {
  accounts: Account[]
  selectedAccountId: string | null
  transactions: AccountTransaction[]
  selectedTransactionIndex: number
  focus: "accounts" | "transactions"
  assigning: boolean
  selectedAssignmentIndex: number
  choosingPosting: boolean
  selectedPostingIndex: number
  editingPostingId: string | null
  creatingAccount: boolean
  newAccountName: string
  newAccountType: "income" | "expense"
  newAccountParentId: string | null
  pageCursors: Array<AccountCursor | null>
  pageIndex: number
  nextCursor: AccountCursor | null
  busy: boolean
  message: string
  error: string | null
  trace: {
    id: number
    event: string
    startedAt: number
    rows: number
  } | null
}

const initialState: State = {
  accounts: [],
  selectedAccountId: null,
  transactions: [],
  selectedTransactionIndex: 0,
  focus: "accounts",
  assigning: false,
  selectedAssignmentIndex: 0,
  choosingPosting: false,
  selectedPostingIndex: 0,
  editingPostingId: null,
  creatingAccount: false,
  newAccountName: "",
  newAccountType: "expense",
  newAccountParentId: null,
  pageCursors: [null],
  pageIndex: 0,
  nextCursor: null,
  busy: false,
  message: "Connecting to Monet",
  error: null,
  trace: null,
}

export const stateAtom = Atom.make(initialState).pipe(Atom.keepAlive)

export const selectedAccountAtom = Atom.make((get) => {
  const state = get(stateAtom)
  return state.accounts.find((account) => account.id === state.selectedAccountId) ?? null
})

const accountTree = (accounts: Account[]): AccountNode[] => {
  const children = new Map<string | null, Account[]>()
  for (const account of accounts) {
    const parent = accounts.some((candidate) => candidate.id === account.parent_id)
      ? account.parent_id
      : null
    children.set(parent, [...(children.get(parent) ?? []), account])
  }
  const result: AccountNode[] = []
  const visit = (parentId: string | null, depth: number) => {
    for (const account of (children.get(parentId) ?? []).sort((a, b) => a.name.localeCompare(b.name))) {
      result.push({ ...account, depth })
      visit(account.id, depth + 1)
    }
  }
  visit(null, 0)
  return result
}

export const accountTreeAtom = Atom.make((get) => accountTree(get(stateAtom).accounts))

export const financialAccountsAtom = Atom.make((get) =>
  get(accountTreeAtom).filter(
    (account) => account.type === "asset" || account.type === "liability",
  ),
)

export const selectableFinancialAccountsAtom = Atom.make((get) =>
  get(financialAccountsAtom),
)

export const selectedTransactionAtom = Atom.make((get) => {
  const state = get(stateAtom)
  return state.transactions[state.selectedTransactionIndex] ?? null
})

export const assignmentAccountsAtom = Atom.make((get) => {
  const state = get(stateAtom)
  return get(accountTreeAtom).filter((account) => account.id !== state.selectedAccountId)
})

export const creationParentsAtom = Atom.make((get) => {
  const state = get(stateAtom)
  return get(accountTreeAtom).filter((account) => account.type === state.newAccountType)
})

const baseUrl = process.env.MONET_SERVER_URL ?? "http://127.0.0.1:47832"
let traceId = 0

export const recordPerformance = (event: string, fields: Record<string, unknown>) => {
  void mkdir(".money-tui", { recursive: true })
    .then(() =>
      appendFile(
        ".money-tui/client-perf.jsonl",
        `${JSON.stringify({ timestamp_ms: Date.now(), event, fields })}\n`,
      ),
    )
    .catch(() => {})
}

const request = <A>(path: string, init?: RequestInit) =>
  Effect.tryPromise({
    try: async () => {
      const started = performance.now()
      const response = await fetch(`${baseUrl}${path}`, init)
      const body = await response.text()
      recordPerformance("frontend.http", {
        path,
        method: init?.method ?? "GET",
        status: response.status,
        bytes: body.length,
        elapsed_ms: performance.now() - started,
      })
      if (!response.ok) {
        throw new Error(body || `${response.status} ${response.statusText}`)
      }
      return JSON.parse(body) as A
    },
    catch: (error) => (error instanceof Error ? error : new Error(String(error))),
  })

const accountPagePath = (accountId: string, cursor: AccountCursor | null) => {
  const query = new URLSearchParams({ limit: "50" })
  if (cursor) {
    query.set("before_date", cursor.date)
    query.set("before_source_id", cursor.source_id)
  }
  return `/accounts/${accountId}/transactions?${query}`
}

const setBusy = (registry: Registry.Registry, message: string) =>
  registry.update(stateAtom, (state) => ({ ...state, busy: true, message, error: null }))

const fail = (registry: Registry.Registry, error: unknown) =>
  registry.update(stateAtom, (state) => ({
    ...state,
    busy: false,
    message: "Request failed",
    error: error instanceof Error ? error.message : String(error),
  }))

const load = (registry: Registry.Registry, preferredAccountId?: string | null, message = "Ready") =>
  Effect.gen(function* () {
    const startedAt = performance.now()
    const accounts = yield* request<Account[]>("/accounts")
    const financialAccounts = accountTree(accounts).filter(
      (account) =>
        account.type === "asset" || account.type === "liability",
    )
    const selectedAccountId =
      financialAccounts.find((account) => account.id === preferredAccountId)?.id ??
      financialAccounts[0]?.id ??
      null
    const page = selectedAccountId
      ? yield* request<AccountPage>(accountPagePath(selectedAccountId, null))
      : { transactions: [], next: null }
    const transactions = page.transactions
    registry.set(stateAtom, {
      accounts,
      selectedAccountId,
      transactions,
      selectedTransactionIndex: 0,
      focus: registry.get(stateAtom).focus,
      assigning: false,
      selectedAssignmentIndex: 0,
      choosingPosting: false,
      selectedPostingIndex: 0,
      editingPostingId: null,
      creatingAccount: false,
      newAccountName: "",
      newAccountType: "expense",
      newAccountParentId: null,
      pageCursors: [null],
      pageIndex: 0,
      nextCursor: page.next,
      busy: false,
      message,
      error: null,
      trace: {
        id: ++traceId,
        event: "load",
        startedAt,
        rows: transactions.length,
      },
    })
  })

const execute = (registry: Registry.Registry, effect: Effect.Effect<void, Error>) => {
  Effect.runFork(effect.pipe(Effect.catchAll((error) => Effect.sync(() => fail(registry, error)))))
}

export const initialize = (registry: Registry.Registry) => {
  setBusy(registry, "Loading accounts")
  execute(registry, load(registry))
}

export const refresh = (registry: Registry.Registry) => {
  const selected = registry.get(stateAtom).selectedAccountId
  setBusy(registry, "Refreshing")
  execute(registry, load(registry, selected))
}

export const selectAccount = (registry: Registry.Registry, accountId: string) => {
  const startedAt = performance.now()
  const state = registry.get(stateAtom)
  if (state.selectedAccountId === accountId || state.busy) return
  registry.update(stateAtom, (current) => ({
    ...current,
    selectedAccountId: accountId,
    transactions: [],
    selectedTransactionIndex: 0,
    focus: "accounts" as const,
    assigning: false,
    choosingPosting: false,
    editingPostingId: null,
    creatingAccount: false,
    pageCursors: [null],
    pageIndex: 0,
    nextCursor: null,
    busy: true,
    message: "Loading transactions",
    error: null,
  }))
  execute(
    registry,
    request<AccountPage>(accountPagePath(accountId, null)).pipe(
      Effect.tap((page) =>
        Effect.sync(() => {
          registry.update(stateAtom, (current) => ({
            ...current,
            transactions: page.transactions,
            nextCursor: page.next,
            busy: false,
            message: "Ready",
            trace: {
              id: ++traceId,
              event: "account_switch",
              startedAt,
              rows: page.transactions.length,
            },
          }))
        }),
      ),
      Effect.asVoid,
    ),
  )
}

export const moveSelection = (registry: Registry.Registry, direction: -1 | 1) => {
  const state = registry.get(stateAtom)
  if (state.assigning) {
    if (state.choosingPosting) {
      const transaction = state.transactions[state.selectedTransactionIndex]
      if (!transaction || transaction.other_postings.length === 0) return
      const next = Math.min(
        transaction.other_postings.length - 1,
        Math.max(0, state.selectedPostingIndex + direction),
      )
      registry.update(stateAtom, (current) => ({ ...current, selectedPostingIndex: next }))
      return
    }
    const accounts = registry.get(assignmentAccountsAtom)
    if (accounts.length === 0) return
    const next = Math.min(
      accounts.length - 1,
      Math.max(0, state.selectedAssignmentIndex + direction),
    )
    registry.update(stateAtom, (current) => ({ ...current, selectedAssignmentIndex: next }))
    return
  }
  if (state.focus === "transactions") {
    if (state.transactions.length === 0) return
    const next = Math.min(
      state.transactions.length - 1,
      Math.max(0, state.selectedTransactionIndex + direction),
    )
    registry.update(stateAtom, (current) => ({ ...current, selectedTransactionIndex: next }))
    return
  }
  const accounts = registry.get(selectableFinancialAccountsAtom)
  if (accounts.length > 0) {
    const current = accounts.findIndex((account) => account.id === state.selectedAccountId)
    const next = Math.min(accounts.length - 1, Math.max(0, current + direction))
    selectAccount(registry, accounts[next].id)
  }
}

export const toggleFocus = (registry: Registry.Registry) => {
  const state = registry.get(stateAtom)
  if (state.assigning) return
  registry.update(stateAtom, (current) => ({
    ...current,
    focus: (current.focus === "accounts" ? "transactions" : "accounts") as State["focus"],
  }))
}

const loadPage = (
  registry: Registry.Registry,
  pageIndex: number,
  cursors: Array<AccountCursor | null>,
) => {
  const state = registry.get(stateAtom)
  const accountId = state.selectedAccountId
  if (!accountId || state.busy) return
  const startedAt = performance.now()
  setBusy(registry, `Loading page ${pageIndex + 1}`)
  execute(
    registry,
    request<AccountPage>(accountPagePath(accountId, cursors[pageIndex] ?? null)).pipe(
      Effect.tap((page) =>
        Effect.sync(() => {
          registry.update(stateAtom, (current) => ({
            ...current,
            transactions: page.transactions,
            selectedTransactionIndex: 0,
            pageCursors: cursors,
            pageIndex,
            nextCursor: page.next,
            busy: false,
            message: "Ready",
            trace: {
              id: ++traceId,
              event: "page_switch",
              startedAt,
              rows: page.transactions.length,
            },
          }))
        }),
      ),
      Effect.asVoid,
    ),
  )
}

export const nextPage = (registry: Registry.Registry) => {
  const state = registry.get(stateAtom)
  if (!state.nextCursor) return
  const cursors = state.pageCursors.slice(0, state.pageIndex + 1)
  cursors.push(state.nextCursor)
  loadPage(registry, state.pageIndex + 1, cursors)
}

export const previousPage = (registry: Registry.Registry) => {
  const state = registry.get(stateAtom)
  if (state.pageIndex === 0) return
  loadPage(registry, state.pageIndex - 1, state.pageCursors)
}

export const selectTransaction = (registry: Registry.Registry, index: number) => {
  registry.update(stateAtom, (state) => ({
    ...state,
    selectedTransactionIndex: index,
    focus: "transactions" as const,
  }))
}

export const beginAssignment = (registry: Registry.Registry) => {
  const state = registry.get(stateAtom)
  const transaction = state.transactions[state.selectedTransactionIndex]
  if (
    !transaction ||
    (transaction.unallocated_amount === 0 && transaction.other_postings.length === 0) ||
    state.busy
  ) return
  const choosingPosting =
    transaction.unallocated_amount === 0 && transaction.other_postings.length > 1
  const editingPosting =
    transaction.unallocated_amount === 0 && transaction.other_postings.length === 1
      ? transaction.other_postings[0]
      : null
  const accounts = registry.get(assignmentAccountsAtom)
  registry.update(stateAtom, (current) => ({
    ...current,
    assigning: true,
    choosingPosting,
    selectedPostingIndex: 0,
    editingPostingId: editingPosting?.posting_id ?? null,
    selectedAssignmentIndex: Math.max(
      0,
      editingPosting
        ? accounts.findIndex((account) => account.id === editingPosting.account_id)
        : 0,
    ),
    message: choosingPosting ? "Choose the posting to change" : "Choose the other account",
  }))
}

export const choosePosting = (registry: Registry.Registry, postingId?: string) => {
  const state = registry.get(stateAtom)
  const transaction = state.transactions[state.selectedTransactionIndex]
  const posting = postingId
    ? transaction?.other_postings.find((candidate) => candidate.posting_id === postingId)
    : transaction?.other_postings[state.selectedPostingIndex]
  if (!posting) return
  const accounts = registry.get(assignmentAccountsAtom)
  registry.update(stateAtom, (current) => ({
    ...current,
    choosingPosting: false,
    editingPostingId: posting.posting_id,
    selectedAssignmentIndex: Math.max(
      0,
      accounts.findIndex((account) => account.id === posting.account_id),
    ),
    message: `Change ${posting.account_name}`,
  }))
}

export const beginAccountCreation = (registry: Registry.Registry) => {
  const state = registry.get(stateAtom)
  if (!state.assigning || state.busy) return
  registry.update(stateAtom, (current) => ({
    ...current,
    creatingAccount: true,
    newAccountName: "",
    newAccountType: "expense" as const,
    newAccountParentId: null,
    message: "Create a category account",
  }))
}

export const cancelAccountCreation = (registry: Registry.Registry) => {
  registry.update(stateAtom, (state) => ({
    ...state,
    creatingAccount: false,
    message: "Choose the other account",
  }))
}

export const setNewAccountName = (registry: Registry.Registry, name: string) => {
  registry.update(stateAtom, (state) => ({ ...state, newAccountName: name }))
}

export const setNewAccountType = (
  registry: Registry.Registry,
  accountType: "income" | "expense",
) => {
  registry.update(stateAtom, (state) => ({
    ...state,
    newAccountType: accountType,
    newAccountParentId: null,
  }))
}

export const setNewAccountParent = (registry: Registry.Registry, parentId: string | null) => {
  registry.update(stateAtom, (state) => ({ ...state, newAccountParentId: parentId }))
}

export const createAccount = (registry: Registry.Registry) => {
  const state = registry.get(stateAtom)
  const name = state.newAccountName.trim()
  if (!name || state.busy) return
  setBusy(registry, `Creating ${name}`)
  execute(
    registry,
    Effect.gen(function* () {
      const account = yield* request<Account>(`/accounts/${crypto.randomUUID()}`, {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          name,
          type: state.newAccountType,
          parent_id: state.newAccountParentId,
        }),
      })
      const accounts = yield* request<Account[]>("/accounts")
      registry.update(stateAtom, (current) => ({
        ...current,
        accounts,
        creatingAccount: false,
        assigning: true,
        selectedAssignmentIndex: Math.max(
          0,
          accountTree(accounts)
            .filter((candidate) => candidate.id !== current.selectedAccountId)
            .findIndex((candidate) => candidate.id === account.id),
        ),
        busy: false,
        message: `Created ${account.name}`,
      }))
    }),
  )
}

export const cancelAssignment = (registry: Registry.Registry) => {
  registry.update(stateAtom, (state) => ({
    ...state,
    assigning: false,
    creatingAccount: false,
    choosingPosting: false,
    editingPostingId: null,
    message: "Ready",
  }))
}

export const assignToAccount = (registry: Registry.Registry, accountId?: string) => {
  const state = registry.get(stateAtom)
  const transaction = state.transactions[state.selectedTransactionIndex]
  const accounts = registry.get(assignmentAccountsAtom)
  const account = accountId
    ? accounts.find((candidate) => candidate.id === accountId)
    : accounts[state.selectedAssignmentIndex]
  if (!transaction || !account || state.busy) return

  setBusy(registry, state.editingPostingId ? `Moving to ${account.name}` : `Assigning to ${account.name}`)
  execute(
    registry,
    Effect.gen(function* () {
      const postingId = state.editingPostingId ?? crypto.randomUUID()
      const changes = state.editingPostingId
        ? [{ operation: "set", target_id: postingId, field: "account_id", value: account.id }]
        : [
            { operation: "split", target_id: postingId },
            { operation: "set", target_id: postingId, field: "account_id", value: account.id },
            {
              operation: "set",
              target_id: postingId,
              field: "amount",
              value: transaction.unallocated_amount,
            },
          ]
      yield* request(`/sources/${transaction.source_id}/changes`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          changes,
        }),
      })
      const cursor = state.pageCursors[state.pageIndex] ?? null
      const page = yield* request<AccountPage>(
        accountPagePath(state.selectedAccountId!, cursor),
      )
      const transactions = page.transactions
      registry.update(stateAtom, (current) => ({
        ...current,
        transactions,
        assigning: false,
        choosingPosting: false,
        editingPostingId: null,
        nextCursor: page.next,
        busy: false,
        message: state.editingPostingId
          ? `Moved to ${account.name}`
          : `Assigned to ${account.name}`,
        trace: {
          id: ++traceId,
          event: "assignment",
          startedAt: performance.now(),
          rows: transactions.length,
        },
      }))
    }),
  )
}

export const sync = (registry: Registry.Registry) => {
  const selected = registry.get(stateAtom).selectedAccountId
  setBusy(registry, "Syncing all accounts")
  execute(
    registry,
    Effect.gen(function* () {
      const summary = yield* request<{
        accounts: number
        transactions: number
        deleted_pending: number
      }>("/import/run", { method: "POST" })
      yield* load(
        registry,
        selected,
        `Synced ${summary.accounts} accounts / ${summary.transactions} transactions`,
      )
    }),
  )
}
