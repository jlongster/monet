import { Atom, type Registry } from "@effect-atom/atom-react"
import { Effect, Fiber } from "effect"
import { appendFile, mkdir } from "node:fs/promises"

export type Account = {
  id: string
  parent_id: string | null
  external_id: string | null
  name: string
  imported_name: string | null
  type: "asset" | "liability" | "income" | "expense" | "equity"
  balance: number
  has_children: boolean
}

export type AccountNode = Account & { depth: number }

export const ALL_TRANSACTIONS_ID = "all-transactions"
export const UNBALANCED_TRANSACTIONS_ID = "unbalanced-transactions"
export const accountTypes = ["asset", "liability", "income", "expense", "equity"] as const
export type AccountView = "assets-liabilities" | "income-expenses"

export type PluginConfig = {
  command: string[]
  cwd: string | null
  env: string[]
  config: unknown
  initializeTimeoutMs: number
  requestTimeoutMs: number
}

export type InferenceConfig = PluginConfig & { model: string | null }
export type ImportPluginConfig = PluginConfig

export type PluginStatus = {
  ready: boolean
  detail?: string | null
  reason?: string | null
  setup?: unknown
  plugin?: { name: string; version: string } | null
  capabilities?: {
    inference: boolean
    transactionImport: boolean
    structuredOutput: boolean
    cancellation: boolean
    payeeFormat?: "clean" | "raw" | null
  } | null
}

export type InferenceModel = {
  id: string
  name: string
  provider?: string | null
  capabilities?: unknown
}

type InferenceUiState = {
  configPath: string | null
  config: InferenceConfig | null
  importConfig: ImportPluginConfig | null
  status: PluginStatus | null
  importStatus: PluginStatus | null
  models: InferenceModel[]
  busy: boolean
  message: string
  error: string | null
  importBusy: boolean
  importMessage: string
  importError: string | null
  testLog: string[]
}

const allTransactionsAccount: AccountNode = {
  id: ALL_TRANSACTIONS_ID,
  parent_id: null,
  external_id: null,
  name: "All transactions",
  imported_name: null,
  type: "asset",
  balance: 0,
  has_children: false,
  depth: 0,
}

const unbalancedTransactionsAccount: AccountNode = {
  ...allTransactionsAccount,
  id: UNBALANCED_TRANSACTIONS_ID,
  name: "Unbalanced",
}

export type AccountTransaction = {
  source_id: string
  posting_id: string
  account_id: string | null
  date: string
  payee: string | null
  amount: number
  pending: boolean
  notes: string | null
  posting_count: number
  unallocated_amount: number
  logs: Array<{
    message: string
    created_at: string
  }>
  other_postings: Array<{
    posting_id: string
    account_id: string | null
    account_name: string | null
    amount: number
  }>
}

export type TransactionChange =
  | { operation: "set"; target_id: string; field: "notes" | "account_id" | "amount"; value: unknown }
  | { operation: "split" | "remove_split"; target_id: string }

type AccountCursor = {
  date: string
  source_id: string
  posting_id: string
}

type AccountPage = {
  transactions: AccountTransaction[]
  next: AccountCursor | null
}

type State = {
  accounts: Account[]
  unbalancedTransactionCount: number
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
  editingAccountId: string | null
  newAccountName: string
  newAccountType: Account["type"]
  newAccountParentId: string | null
  nextCursor: AccountCursor | null
  loadingMore: boolean
  pendingTransactionIndex: number | null
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
  unbalancedTransactionCount: 0,
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
  editingAccountId: null,
  newAccountName: "",
  newAccountType: "asset",
  newAccountParentId: null,
  nextCursor: null,
  loadingMore: false,
  pendingTransactionIndex: null,
  busy: false,
  message: "Connecting to Monet",
  error: null,
  trace: null,
}

export const stateAtom = Atom.make(initialState).pipe(Atom.keepAlive)
export const collapsedAccountIdsAtom = Atom.make(new Set<string>() as ReadonlySet<string>).pipe(Atom.keepAlive)
export const accountViewAtom = Atom.make("assets-liabilities" as AccountView).pipe(Atom.keepAlive)
export const transactionSearchAtom = Atom.make("").pipe(Atom.keepAlive)
export const syncLogAtom = Atom.make({ running: false, lines: [] as string[] }).pipe(Atom.keepAlive)
export const inferenceAtom = Atom.make({
  configPath: null,
  config: null,
  importConfig: null,
  status: null,
  importStatus: null,
  models: [],
  busy: false,
  message: "Inference is not configured",
  error: null,
  importBusy: false,
  importMessage: "Transaction import is not configured",
  importError: null,
  testLog: [],
} as InferenceUiState).pipe(Atom.keepAlive)

export const selectedAccountAtom = Atom.make((get) => {
  const state = get(stateAtom)
  if (state.selectedAccountId === ALL_TRANSACTIONS_ID) return allTransactionsAccount
  if (state.selectedAccountId === UNBALANCED_TRANSACTIONS_ID) return unbalancedTransactionsAccount
  return state.accounts.find((account) => account.id === state.selectedAccountId) ?? null
})

const accountTree = (
  accounts: Account[],
  collapsedAccountIds: ReadonlySet<string> = new Set(),
): AccountNode[] => {
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
      if (!collapsedAccountIds.has(account.id)) visit(account.id, depth + 1)
    }
  }
  visit(null, 0)
  return result
}

const financialAccountTree = (
  accounts: Account[],
  unbalancedTransactionCount: number,
  collapsedAccountIds: ReadonlySet<string>,
  accountView: AccountView,
) => [
  allTransactionsAccount,
  ...(unbalancedTransactionCount > 0
    ? [{ ...unbalancedTransactionsAccount, name: `Unbalanced (${unbalancedTransactionCount})` }]
    : []),
  ...accountTree(
    accounts.filter((account) => accountView === "income-expenses"
      ? account.type === "income" || account.type === "expense"
      : account.type === "asset" || account.type === "liability" || account.type === "equity"),
    collapsedAccountIds,
  ),
]

export const accountTreeAtom = Atom.make((get) => accountTree(get(stateAtom).accounts))

export const financialAccountsAtom = Atom.make((get) =>
  financialAccountTree(
    get(stateAtom).accounts,
    get(stateAtom).unbalancedTransactionCount,
    get(collapsedAccountIdsAtom),
    get(accountViewAtom),
  ),
)

export const selectableFinancialAccountsAtom = Atom.make((get) =>
  get(financialAccountsAtom),
)

export const selectedTransactionAtom = Atom.make((get) => {
  const state = get(stateAtom)
  return state.transactions[state.selectedTransactionIndex] ?? null
})

const availableAssignmentAccounts = (accounts: Account[], state: State) => {
  const transactionAccountId = state.transactions[state.selectedTransactionIndex]?.account_id
  return accountTree(accounts).filter(
    (account) =>
      account.id !== state.selectedAccountId && account.id !== transactionAccountId,
  )
}

export const assignmentAccountsAtom = Atom.make((get) => {
  const state = get(stateAtom)
  return availableAssignmentAccounts(state.accounts, state)
})

export const creationParentsAtom = Atom.make((get) => {
  return get(accountTreeAtom)
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

const TRANSACTION_PAGE_SIZE = 100
const TRANSACTION_BUFFER_AHEAD = 150

const accountTypesForView = (accountView: AccountView) => accountView === "income-expenses"
  ? "income,expense"
  : "asset,liability,equity"

const accountPagePath = (
  accountId: string,
  cursor: AccountCursor | null,
  accountView: AccountView,
  search: string,
) => {
  const query = new URLSearchParams({ limit: String(TRANSACTION_PAGE_SIZE) })
  if (accountId === ALL_TRANSACTIONS_ID || accountId === UNBALANCED_TRANSACTIONS_ID) {
    query.set("account_types", accountTypesForView(accountView))
  }
  if (search.trim()) query.set("search", search.trim())
  if (cursor) {
    query.set("before_date", cursor.date)
    query.set("before_source_id", cursor.source_id)
    query.set("before_posting_id", cursor.posting_id)
  }
  const path = accountId === ALL_TRANSACTIONS_ID
    ? "/transactions"
    : accountId === UNBALANCED_TRANSACTIONS_ID
      ? "/transactions/unbalanced"
      : `/accounts/${accountId}/transactions`
  return `${path}?${query}`
}

const setBusy = (registry: Registry.Registry, message: string) =>
  registry.update(stateAtom, (state) => ({
    ...state,
    busy: true,
    loadingMore: false,
    pendingTransactionIndex: null,
    message,
    error: null,
  }))

const fail = (registry: Registry.Registry, error: unknown) =>
  registry.update(stateAtom, (state) => ({
    ...state,
    busy: false,
    loadingMore: false,
    pendingTransactionIndex: null,
    message: "Request failed",
    error: error instanceof Error ? error.message : String(error),
  }))

const load = (registry: Registry.Registry, preferredAccountId?: string | null, message = "Ready") =>
  Effect.gen(function* () {
    const startedAt = performance.now()
    const accounts = yield* request<Account[]>("/accounts")
    const accountView = registry.get(accountViewAtom)
    const search = registry.get(transactionSearchAtom)
    const unbalancedQuery = new URLSearchParams({ account_types: accountTypesForView(accountView) })
    if (search.trim()) unbalancedQuery.set("search", search.trim())
    const unbalanced = yield* request<{ count: number }>(
      `/transactions/unbalanced/count?${unbalancedQuery}`,
    )
    const financialAccounts = financialAccountTree(
      accounts,
      unbalanced.count,
      registry.get(collapsedAccountIdsAtom),
      accountView,
    )
    const selectedAccountId =
      financialAccounts.find((account) => account.id === preferredAccountId)?.id ??
      financialAccounts[0]?.id ??
      null
    const page = selectedAccountId
      ? yield* request<AccountPage>(accountPagePath(selectedAccountId, null, accountView, search))
      : { transactions: [], next: null }
    const transactions = page.transactions
    registry.set(stateAtom, {
      accounts,
      unbalancedTransactionCount: unbalanced.count,
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
      editingAccountId: null,
      newAccountName: "",
      newAccountType: "asset",
      newAccountParentId: null,
      nextCursor: page.next,
      loadingMore: false,
      pendingTransactionIndex: null,
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
    ensureTransactionBuffer(registry)
  })

const execute = (registry: Registry.Registry, effect: Effect.Effect<void, Error>) => {
  Effect.runFork(effect.pipe(Effect.catchAll((error) => Effect.sync(() => fail(registry, error)))))
}

const executeInference = (
  registry: Registry.Registry,
  effect: Effect.Effect<void, Error>,
  onError?: (error: Error) => void,
) => {
  Effect.runFork(effect.pipe(Effect.catchAll((error) => Effect.sync(() => {
    if (onError) {
      onError(error)
      return
    }
    registry.update(inferenceAtom, (state) => ({
      ...state,
      busy: false,
      message: "Inference request failed",
      error: error.message,
    }))
  }))))
}

const refreshInference = (registry: Registry.Registry, config: InferenceConfig) =>
  Effect.gen(function* () {
    const status = yield* request<PluginStatus>("/inference/status")
    const response = yield* request<{ models: InferenceModel[] }>("/inference/models")
    registry.update(inferenceAtom, (current) => ({
      ...current,
      config,
      status,
      models: response.models,
      busy: false,
      message: status.ready ? "Inference is ready" : (status.reason ?? "Inference needs setup"),
      error: null,
    }))
  })

export const loadInference = (registry: Registry.Registry) => {
  registry.update(inferenceAtom, (state) => ({ ...state, busy: true, error: null, message: "Loading inference" }))
  executeInference(
    registry,
    Effect.gen(function* () {
      const response = yield* request<{
        path: string
        inference: InferenceConfig | null
        import: ImportPluginConfig | null
      }>("/plugins")
      registry.update(inferenceAtom, (state) => ({
        ...state,
        configPath: response.path,
        importConfig: response.import,
      }))
      if (!response.inference) {
        registry.update(inferenceAtom, (state) => ({
          ...state,
          config: null,
          status: null,
          models: [],
          busy: false,
          message: `Configure inference in ${response.path}`,
          error: null,
        }))
        return
      }
      yield* refreshInference(registry, response.inference)
    }),
  )
}

const executeImport = (registry: Registry.Registry, effect: Effect.Effect<void, Error>) => {
  Effect.runFork(effect.pipe(Effect.catchAll((error) => Effect.sync(() => {
    registry.update(inferenceAtom, (state) => ({
      ...state,
      importBusy: false,
      importMessage: "Transaction import request failed",
      importError: error.message,
    }))
  }))))
}

export const loadImportPlugin = (registry: Registry.Registry) => {
  registry.update(inferenceAtom, (state) => ({
    ...state,
    importBusy: true,
    importError: null,
    importMessage: "Loading transaction import",
  }))
  executeImport(
    registry,
    Effect.gen(function* () {
      const response = yield* request<{
        path: string
        inference: InferenceConfig | null
        import: ImportPluginConfig | null
      }>("/plugins")
      if (!response.import) {
        registry.update(inferenceAtom, (state) => ({
          ...state,
          configPath: response.path,
          importConfig: null,
          importStatus: null,
          importBusy: false,
          importMessage: `Configure transaction import in ${response.path}`,
          importError: null,
        }))
        return
      }
      const status = yield* request<PluginStatus>("/import/status")
      registry.update(inferenceAtom, (state) => ({
        ...state,
        configPath: response.path,
        importConfig: response.import,
        importStatus: status,
        importBusy: false,
        importMessage: status.ready ? "Transaction import is ready" : (status.reason ?? "Transaction import needs setup"),
        importError: null,
      }))
    }),
  )
}

export const restartImportPlugin = (registry: Registry.Registry) => {
  registry.update(inferenceAtom, (state) => ({
    ...state,
    importBusy: true,
    importError: null,
    importMessage: "Restarting transaction import",
  }))
  executeImport(
    registry,
    Effect.gen(function* () {
      yield* request<{ healthy: boolean }>("/import/restart", { method: "POST" })
      loadImportPlugin(registry)
    }),
  )
}

export const testInference = (registry: Registry.Registry, prompt: string, model?: string) => {
  const startedAt = performance.now()
  const state = registry.get(inferenceAtom)
  const requestedModel = model || state.config?.model || "default"
  registry.update(inferenceAtom, (current) => ({
    ...current,
    busy: true,
    error: null,
    message: "Testing inference",
    testLog: [
      ...current.testLog,
      ...(current.testLog.length > 0 ? [""] : []),
      "Starting test generation",
      `Plugin: ${current.status?.plugin?.name ?? "unknown"}`,
      `Requested model: ${requestedModel}`,
      "Sending inference/generate request",
    ],
  }))
  executeInference(
    registry,
    Effect.gen(function* () {
      const response = yield* request<{ text: string; model: string }>("/inference/generate", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ prompt, model: model || undefined }),
      })
      registry.update(inferenceAtom, (state) => ({
        ...state,
        busy: false,
        message: "Inference test complete",
        error: null,
        testLog: [
          ...state.testLog,
          `Response received (${Math.round(performance.now() - startedAt)} ms)`,
          `Resolved model: ${response.model}`,
          "Test generation complete",
          `Output: ${response.text}`,
        ],
      }))
    }),
    (error) => {
      registry.update(inferenceAtom, (current) => ({
        ...current,
        busy: false,
        message: "Inference request failed",
        error: error.message,
        testLog: [
          ...current.testLog,
          `Request failed (${Math.round(performance.now() - startedAt)} ms)`,
          `Error: ${error.message}`,
        ],
      }))
    },
  )
}

export const saveInferenceModel = (registry: Registry.Registry, model: string) => {
  registry.update(inferenceAtom, (state) => ({
    ...state,
    busy: true,
    error: null,
    message: "Saving inference model",
  }))
  executeInference(
    registry,
    Effect.gen(function* () {
      const config = yield* request<InferenceConfig>("/inference/model", {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ model }),
      })
      registry.update(inferenceAtom, (state) => ({
        ...state,
        config,
        busy: false,
        message: "Inference model saved",
        error: null,
      }))
    }),
  )
}

export const restartInference = (registry: Registry.Registry) => {
  registry.update(inferenceAtom, (state) => ({ ...state, busy: true, error: null, message: "Restarting inference" }))
  executeInference(
    registry,
    Effect.gen(function* () {
      yield* request<{ healthy: boolean }>("/inference/restart", { method: "POST" })
      const response = yield* request<{
        path: string
        inference: InferenceConfig | null
        import: ImportPluginConfig | null
      }>("/plugins")
      if (response.inference) {
        registry.update(inferenceAtom, (state) => ({ ...state, configPath: response.path }))
        yield* refreshInference(registry, response.inference)
      } else {
        registry.update(inferenceAtom, (state) => ({
          ...state,
          configPath: response.path,
          config: null,
          status: null,
          models: [],
          busy: false,
          message: `Configure inference in ${response.path}`,
          error: null,
        }))
      }
    }),
  )
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
  const accountView = registry.get(accountViewAtom)
  const search = registry.get(transactionSearchAtom)
  const state = registry.get(stateAtom)
  if (state.busy) return
  if (state.selectedAccountId === accountId) {
    registry.update(stateAtom, (current) => ({ ...current, focus: "accounts" as const }))
    return
  }
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
    editingAccountId: null,
    nextCursor: null,
    loadingMore: false,
    pendingTransactionIndex: null,
    busy: true,
    message: "Loading transactions",
    error: null,
  }))
  execute(
    registry,
    request<AccountPage>(accountPagePath(accountId, null, accountView, search)).pipe(
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
          ensureTransactionBuffer(registry)
        }),
      ),
      Effect.asVoid,
    ),
  )
}

export const setAccountView = (registry: Registry.Registry, accountView: AccountView) => {
  if (registry.get(accountViewAtom) === accountView) return
  const state = registry.get(stateAtom)
  if (state.busy) return
  registry.set(accountViewAtom, accountView)
  const visibleAccounts = registry.get(financialAccountsAtom)
  const selectedAccountId = visibleAccounts.some((account) => account.id === state.selectedAccountId)
    ? state.selectedAccountId
    : visibleAccounts[0]?.id ?? ALL_TRANSACTIONS_ID
  setBusy(registry, "Switching account view")
  execute(registry, load(registry, selectedAccountId))
}

export const setTransactionSearch = (registry: Registry.Registry, search: string) => {
  if (registry.get(transactionSearchAtom) === search) return
  const state = registry.get(stateAtom)
  if (state.busy) return
  registry.set(transactionSearchAtom, search)
  setBusy(registry, search.trim() ? "Searching transactions" : "Loading transactions")
  execute(registry, load(registry, state.selectedAccountId))
}

export const setAccountCollapsed = (
  registry: Registry.Registry,
  accountId: string,
  collapsed: boolean,
) => {
  registry.update(collapsedAccountIdsAtom, (current) => {
    const next = new Set(current)
    if (collapsed) next.add(accountId)
    else next.delete(accountId)
    return next
  })
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
    selectBufferedTransaction(registry, state.selectedTransactionIndex + direction)
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

const cursorsEqual = (left: AccountCursor | null, right: AccountCursor | null) =>
  left?.date === right?.date && left?.source_id === right?.source_id

const loadMoreTransactions = (registry: Registry.Registry) => {
  const state = registry.get(stateAtom)
  const accountId = state.selectedAccountId
  const accountView = registry.get(accountViewAtom)
  const search = registry.get(transactionSearchAtom)
  const cursor = state.nextCursor
  if (!accountId || !cursor || state.busy || state.loadingMore) return
  const startedAt = performance.now()
  registry.update(stateAtom, (current) => ({ ...current, loadingMore: true }))
  execute(
    registry,
    request<AccountPage>(accountPagePath(accountId, cursor, accountView, search)).pipe(
      Effect.tap((page) =>
        Effect.sync(() => {
          registry.update(stateAtom, (current) => {
            if (
              current.selectedAccountId !== accountId ||
              current.busy ||
              !cursorsEqual(current.nextCursor, cursor)
            ) return current
            const knownPostingIds = new Set(current.transactions.map((transaction) => transaction.posting_id))
            const transactions = [
              ...current.transactions,
              ...page.transactions.filter((transaction) => !knownPostingIds.has(transaction.posting_id)),
            ]
            const pendingIndex = current.pendingTransactionIndex
            const reachedPendingIndex = pendingIndex !== null &&
              (pendingIndex < transactions.length || page.next === null)
            return {
              ...current,
              transactions,
              selectedTransactionIndex: reachedPendingIndex
                ? Math.min(pendingIndex, Math.max(0, transactions.length - 1))
                : current.selectedTransactionIndex,
              pendingTransactionIndex: reachedPendingIndex ? null : pendingIndex,
              nextCursor: page.next,
              loadingMore: false,
              message: "Ready",
              trace: {
                id: ++traceId,
                event: "buffer_append",
                startedAt,
                rows: page.transactions.length,
              },
            }
          })
          ensureTransactionBuffer(registry)
        }),
      ),
      Effect.asVoid,
    ),
  )
}

const ensureTransactionBuffer = (registry: Registry.Registry) => {
  const state = registry.get(stateAtom)
  if (state.busy || state.loadingMore || !state.nextCursor) return
  const rowsAhead = state.transactions.length - state.selectedTransactionIndex - 1
  if (state.pendingTransactionIndex !== null || rowsAhead < TRANSACTION_BUFFER_AHEAD) {
    loadMoreTransactions(registry)
  }
}

export const selectBufferedTransaction = (registry: Registry.Registry, targetIndex: number) => {
  const state = registry.get(stateAtom)
  if (state.transactions.length === 0) return
  const target = Math.max(0, targetIndex)
  if (target < state.transactions.length) {
    registry.update(stateAtom, (current) => ({
      ...current,
      selectedTransactionIndex: target,
      pendingTransactionIndex: null,
    }))
    ensureTransactionBuffer(registry)
    return
  }
  if (!state.nextCursor) {
    registry.update(stateAtom, (current) => ({
      ...current,
      selectedTransactionIndex: current.transactions.length - 1,
      pendingTransactionIndex: null,
    }))
    return
  }
  registry.update(stateAtom, (current) => ({
    ...current,
    pendingTransactionIndex: target,
  }))
  ensureTransactionBuffer(registry)
}

const fetchTransactionBuffer = (
  accountId: string,
  minimumRows: number,
  accountView: AccountView,
  search: string,
) =>
  Effect.gen(function* () {
    const transactions: AccountTransaction[] = []
    let cursor: AccountCursor | null = null
    let nextCursor: AccountCursor | null = null
    do {
      const page: AccountPage = yield* request<AccountPage>(
        accountPagePath(accountId, cursor, accountView, search),
      )
      transactions.push(...page.transactions)
      nextCursor = page.next
      cursor = page.next
    } while (cursor && transactions.length < minimumRows)
    return { transactions, nextCursor }
  })

export const selectTransaction = (registry: Registry.Registry, index: number) => {
  registry.update(stateAtom, (state) => ({
    ...state,
    selectedTransactionIndex: index,
    pendingTransactionIndex: null,
    focus: "transactions" as const,
  }))
  ensureTransactionBuffer(registry)
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
  if (state.busy) return
  const parent = state.accounts.find((account) => account.id === state.selectedAccountId) ?? null
  registry.update(stateAtom, (current) => ({
    ...current,
    creatingAccount: true,
    editingAccountId: null,
    assigning: false,
    newAccountName: "",
    newAccountType: parent?.type ?? "asset" as const,
    newAccountParentId: parent?.id ?? null,
    selectedAssignmentIndex: 0,
    message: parent ? `Create account under ${parent.name}` : "Choose a parent account",
  }))
}

export const cancelAccountCreation = (registry: Registry.Registry) => {
  registry.update(stateAtom, (state) => ({
    ...state,
    creatingAccount: false,
    editingAccountId: null,
    newAccountName: "",
    newAccountParentId: null,
    message: "Ready",
  }))
}

export const beginAccountEditing = (registry: Registry.Registry) => {
  const state = registry.get(stateAtom)
  const account = state.accounts.find((candidate) => candidate.id === state.selectedAccountId)
  if (!account || state.busy) return
  registry.update(stateAtom, (current) => ({
    ...current,
    creatingAccount: true,
    editingAccountId: account.id,
    assigning: false,
    newAccountName: account.name,
    newAccountType: account.type,
    newAccountParentId: account.parent_id,
    message: `Editing ${account.name}`,
  }))
}

export const setNewAccountName = (registry: Registry.Registry, name: string) => {
  registry.update(stateAtom, (state) => ({ ...state, newAccountName: name }))
}

export const setNewAccountParent = (
  registry: Registry.Registry,
  parentId: string | null,
) => {
  const state = registry.get(stateAtom)
  if (parentId === null) {
    registry.update(stateAtom, (current) => ({
      ...current,
      newAccountParentId: null,
      message: state.editingAccountId ? "Move account to top level" : "Creating root account",
    }))
    return
  }
  const parent = state.accounts.find((account) => account.id === parentId)
  if (!parent) return
  registry.update(stateAtom, (current) => ({
    ...current,
    newAccountParentId: parent.id,
    newAccountType: parent.type,
    message: state.editingAccountId ? `Move account under ${parent.name}` : `Create account under ${parent.name}`,
  }))
}

export const setNewAccountType = (
  registry: Registry.Registry,
  accountType: Account["type"],
) => {
  const editing = registry.get(stateAtom).editingAccountId !== null
  registry.update(stateAtom, (current) => ({
    ...current,
    newAccountType: accountType,
    message: editing ? `Change account type to ${accountType}` : `Create ${accountType} account`,
  }))
}

export const createAccount = (registry: Registry.Registry) => {
  const state = registry.get(stateAtom)
  const name = state.newAccountName.trim()
  if (!name || state.busy) return
  const editing = state.editingAccountId !== null
  const accountId = state.editingAccountId ?? crypto.randomUUID()
  setBusy(registry, editing ? `Saving ${name}` : `Creating ${name}`)
  execute(
    registry,
    Effect.gen(function* () {
      const account = yield* request<Account>(`/accounts/${accountId}`, {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          name,
          type: state.newAccountType,
          parent_id: state.newAccountParentId,
        }),
      })
      if (editing) {
        yield* load(registry, account.id, `Saved ${account.name}`)
        return
      }
      const accounts = yield* request<Account[]>("/accounts")
      registry.update(stateAtom, (current) => ({
        ...current,
        accounts,
        creatingAccount: false,
        editingAccountId: null,
        assigning: false,
        newAccountName: "",
        newAccountParentId: null,
        busy: false,
        message: `Created ${account.name}`,
      }))
    }),
  )
}

const debugCategoryAccounts: ReadonlyArray<{
  id: string
  parent_id: string | null
  name: string
  type: "expense" | "income"
}> = [
  { id: "019fd900-0000-7000-8000-000000000001", parent_id: null, name: "Expense", type: "expense" },
  { id: "019fd900-0000-7000-8000-000000000002", parent_id: "019fd900-0000-7000-8000-000000000001", name: "Life", type: "expense" },
  { id: "019fd900-0000-7000-8000-000000000003", parent_id: "019fd900-0000-7000-8000-000000000001", name: "Food", type: "expense" },
  { id: "019fd900-0000-7000-8000-000000000004", parent_id: "019fd900-0000-7000-8000-000000000001", name: "Tithe", type: "expense" },
  { id: "019fd900-0000-7000-8000-000000000005", parent_id: "019fd900-0000-7000-8000-000000000001", name: "Kids", type: "expense" },
  { id: "019fd900-0000-7000-8000-000000000006", parent_id: "019fd900-0000-7000-8000-000000000001", name: "Clothing", type: "expense" },
  { id: "019fd900-0000-7000-8000-000000000007", parent_id: null, name: "Income", type: "income" },
  { id: "019fd900-0000-7000-8000-000000000008", parent_id: "019fd900-0000-7000-8000-000000000007", name: "Salary", type: "income" },
  { id: "019fd900-0000-7000-8000-000000000009", parent_id: "019fd900-0000-7000-8000-000000000007", name: "Misc", type: "income" },
]

export const createDebugCategories = (registry: Registry.Registry) => {
  const state = registry.get(stateAtom)
  if (state.busy) return
  setBusy(registry, "Creating debug categories")
  execute(
    registry,
    Effect.gen(function* () {
      for (const account of debugCategoryAccounts) {
        yield* request<Account>(`/accounts/${account.id}`, {
          method: "PUT",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            name: account.name,
            type: account.type,
            parent_id: account.parent_id,
          }),
        })
      }
      yield* load(registry, state.selectedAccountId, "Created debug categories")
    }),
  )
}

export const deleteAccount = (
  registry: Registry.Registry,
  accountId: string,
  deleteTransactions: boolean,
) => {
  const state = registry.get(stateAtom)
  if (state.busy) return
  setBusy(registry, "Deleting account")
  execute(
    registry,
    Effect.gen(function* () {
      const query = new URLSearchParams({
        delete_transactions: String(deleteTransactions),
      })
      const deletion = yield* request<{
        accounts: number
        transactions: number
        nulled_postings: number
      }>(`/accounts/${accountId}?${query}`, { method: "DELETE" })
      yield* load(
        registry,
        UNBALANCED_TRANSACTIONS_ID,
        deleteTransactions
          ? `Deleted ${deletion.accounts} accounts / ${deletion.transactions} transactions`
          : `Deleted ${deletion.accounts} accounts / cleared ${deletion.nulled_postings} postings`,
      )
    }),
  )
}

export const cancelAssignment = (registry: Registry.Registry) => {
  registry.update(stateAtom, (state) => ({
    ...state,
    assigning: false,
    creatingAccount: false,
    editingAccountId: null,
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
      const buffer = yield* fetchTransactionBuffer(
        state.selectedAccountId!,
        Math.max(state.transactions.length, TRANSACTION_PAGE_SIZE * 2),
        registry.get(accountViewAtom),
        registry.get(transactionSearchAtom),
      )
      const selectedIndex = buffer.transactions.findIndex(
        (candidate) => candidate.source_id === transaction.source_id,
      )
      registry.update(stateAtom, (current) => ({
        ...current,
        transactions: buffer.transactions,
        selectedTransactionIndex: selectedIndex >= 0
          ? selectedIndex
          : Math.min(current.selectedTransactionIndex, Math.max(0, buffer.transactions.length - 1)),
        assigning: false,
        choosingPosting: false,
        editingPostingId: null,
        nextCursor: buffer.nextCursor,
        loadingMore: false,
        pendingTransactionIndex: null,
        busy: false,
        message: state.editingPostingId
          ? `Moved to ${account.name}`
          : `Assigned to ${account.name}`,
        trace: {
          id: ++traceId,
          event: "assignment",
          startedAt: performance.now(),
          rows: buffer.transactions.length,
        },
      }))
      ensureTransactionBuffer(registry)
    }),
  )
}

export const setInlineCounterpart = (
  registry: Registry.Registry,
  accountId: string,
) => {
  const state = registry.get(stateAtom)
  const transaction = state.transactions[state.selectedTransactionIndex]
  if (!transaction || state.busy || transaction.other_postings.length > 1) return
  const existing = transaction.other_postings[0]
  if (!existing && transaction.unallocated_amount === 0) return
  setBusy(registry, "Updating counterpart")
  execute(
    registry,
    Effect.gen(function* () {
      const postingId = existing?.posting_id ?? crypto.randomUUID()
      const changes = existing
        ? [{ operation: "set", target_id: postingId, field: "account_id", value: accountId }]
        : [
            { operation: "split", target_id: postingId },
            { operation: "set", target_id: postingId, field: "account_id", value: accountId },
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
        body: JSON.stringify({ changes }),
      })
      const buffer = yield* fetchTransactionBuffer(
        state.selectedAccountId!,
        Math.max(state.transactions.length, TRANSACTION_PAGE_SIZE * 2),
        registry.get(accountViewAtom),
        registry.get(transactionSearchAtom),
      )
      const selectedIndex = buffer.transactions.findIndex(
        (candidate) =>
          candidate.source_id === transaction.source_id &&
          candidate.posting_id === transaction.posting_id,
      )
      registry.update(stateAtom, (current) => ({
        ...current,
        transactions: buffer.transactions,
        selectedTransactionIndex: selectedIndex >= 0
          ? selectedIndex
          : Math.max(0, buffer.transactions.findIndex(
            (candidate) => candidate.source_id === transaction.source_id,
          )),
        nextCursor: buffer.nextCursor,
        busy: false,
        message: "Updated counterpart",
      }))
      ensureTransactionBuffer(registry)
    }),
  )
}

export const saveTransactionChanges = (
  registry: Registry.Registry,
  sourceId: string,
  changes: TransactionChange[],
  onSuccess: () => void,
) => {
  const state = registry.get(stateAtom)
  if (state.busy) return
  setBusy(registry, "Saving transaction")
  execute(
    registry,
    Effect.gen(function* () {
      yield* request(`/sources/${sourceId}/changes`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ changes }),
      })
      const buffer = yield* fetchTransactionBuffer(
        state.selectedAccountId!,
        Math.max(state.transactions.length, TRANSACTION_PAGE_SIZE * 2),
        registry.get(accountViewAtom),
        registry.get(transactionSearchAtom),
      )
      const selectedIndex = buffer.transactions.findIndex(
        (transaction) => transaction.source_id === sourceId,
      )
      registry.update(stateAtom, (current) => ({
        ...current,
        transactions: buffer.transactions,
        selectedTransactionIndex: selectedIndex >= 0
          ? selectedIndex
          : Math.min(current.selectedTransactionIndex, Math.max(0, buffer.transactions.length - 1)),
        nextCursor: buffer.nextCursor,
        loadingMore: false,
        pendingTransactionIndex: null,
        busy: false,
        message: "Saved transaction",
      }))
      ensureTransactionBuffer(registry)
      onSuccess()
    }),
  )
}

export type SyncOptions = {
  endDate: string
}

export const sync = (
  registry: Registry.Registry,
  options: SyncOptions,
) => {
  const current = registry.get(stateAtom)
  if (current.busy || registry.get(syncLogAtom).running) return
  registry.set(syncLogAtom, { running: true, lines: ["Starting transaction sync"] })
  registry.update(stateAtom, (state) => ({
    ...state,
    message: "Syncing all accounts in background",
    error: null,
  }))
  const refreshLog = request<{ running: boolean; lines: string[] }>("/import/logs").pipe(
    Effect.tap((log) => Effect.sync(() => registry.set(syncLogAtom, log))),
    Effect.catchAll(() => Effect.void),
  )
  const polling = Effect.runFork(
    Effect.forever(refreshLog.pipe(Effect.zipRight(Effect.sleep(300)))),
  )
  execute(
    registry,
    Effect.gen(function* () {
      const summary = yield* request<{
        accounts: number
        transactions: number
        deleted_pending: number
        categorized: number
        payees_resolved: number
      }>("/import/run", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(options),
      })
      yield* refreshLog
      yield* load(
        registry,
        registry.get(stateAtom).selectedAccountId,
        `Synced ${summary.accounts} accounts / ${summary.transactions} transactions / resolved ${summary.payees_resolved} payees / categorized ${summary.categorized}`,
      )
    }).pipe(
      Effect.ensuring(Fiber.interrupt(polling).pipe(Effect.zipRight(refreshLog))),
    ),
  )
}
