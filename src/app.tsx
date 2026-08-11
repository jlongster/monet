import { RegistryContext, useAtomValue } from "@effect-atom/atom-react"
import type { BoxRenderable, Renderable, ScrollBoxRenderable, TextareaRenderable, TextRenderable } from "@opentui/core"
import { useKeyboard, useRenderer, useTerminalDimensions } from "@opentui/react"
import { useContext, useEffect, useRef, useState } from "react"
import stringWidth from "string-width"

import {
  ALL_TRANSACTIONS_ID,
  UNBALANCED_TRANSACTIONS_ID,
  type Account,
  type AccountView,
  type TransactionChange,
  accountViewAtom,
  accountTypes,
  assignmentAccountsAtom,
  assignToAccount,
  beginAccountCreation,
  beginAccountEditing,
  cancelAccountCreation,
  cancelAssignment,
  choosePosting,
  collapsedAccountIdsAtom,
  createAccount,
  createDebugCategories,
  deleteAccount,
  creationParentsAtom,
  financialAccountsAtom,
  initialize,
  moveSelection,
  recordPerformance,
  refresh,
  selectAccount,
  selectBufferedTransaction,
  selectTransaction,
  setAccountCollapsed,
  setAccountView,
  setInlineCounterpart,
  setInlinePayee,
  setTransactionSearch,
  selectedAccountAtom,
  selectedTransactionAtom,
  saveTransactionChanges,
  setNewAccountName,
  setNewAccountParent,
  setNewAccountType,
  stateAtom,
  sync,
  syncLogAtom,
  toggleFocus,
  transactionSearchAtom,
} from "./store"
import { FilterSelect, type FilterSelectOption } from "./filter-select"
import { DebugSync } from "./debug-sync"
import { moveOrderedFocus } from "./focus-order"
import { FormButton, FormField } from "./form"
import { PluginsSettings } from "./plugins-settings"
import { Popover, PopoverProvider } from "./popover"
import { scrollTopForRange } from "./selection-scroll"
import { gray, selectionBackground } from "./theme"

const NO_PARENT = "__no_parent__"
const NO_ACCOUNT = "__no_account__"
const TRANSACTION_EDITOR_CONTENT_WIDTH = 33
const TRANSACTION_EDITOR_TEXT_WIDTH = TRANSACTION_EDITOR_CONTENT_WIDTH - 2
const ACCOUNT_SIDEBAR_WIDTH = 28
const TRANSACTION_EDITOR_WIDTH = 36
const MIN_TRANSACTION_PANE_WIDTH = 52
const TRANSACTION_TABLE_WITH_ACCOUNT_WIDTH = 68
const TRANSACTION_TABLE_WITH_COUNTERPART_WIDTH = 56

type AllocationDraft = {
  postingId: string
  accountId: string | null
  amount: string
  existing: boolean
}

type TransactionDraft = {
  sourceId: string
  sourceAccountId: string | null
  sourceAmount: number
  originalPostingIds: string[]
  allocations: AllocationDraft[]
}

const money = (amount: number) => {
  const absolute = Math.abs(amount)
  const formatted = `$${Math.floor(absolute / 100).toLocaleString()}.${String(absolute % 100).padStart(2, "0")}`
  return amount < 0 ? `-${formatted}` : formatted
}

const accountBalance = (account: Account) =>
  account.type === "income" || account.type === "liability" || account.type === "equity"
    ? -account.balance
    : account.balance

const AccountAmountRow = ({
  name,
  amount,
  amountWidth,
  muted = false,
  bright = false,
}: {
  name: string
  amount: string
  amountWidth: number
  muted?: boolean
  bright?: boolean
}) => (
  <box style={{ width: "100%", height: 1, flexDirection: "row", gap: 1 }}>
    <text fg={muted ? gray[500] : bright ? gray[100] : gray[300]} wrapMode="none" style={{ flexGrow: 1, minWidth: 0 }}>
      {short(name, Math.max(1, TRANSACTION_EDITOR_CONTENT_WIDTH - amountWidth - 1))}
    </text>
    <box style={{ width: amountWidth, height: 1, flexDirection: "row", justifyContent: "flex-end" }}>
      <text fg={muted ? gray[300] : gray[200]}>{amount}</text>
    </box>
  </box>
)

const short = (value: string | null, width: number) => {
  const text = value || "(no payee)"
  return text.length > width ? `${text.slice(0, width - 1)}…` : text
}

const flexibleTableColumn = { flexGrow: 1, flexBasis: 0, minWidth: 0 } as const

const decimalAmount = (amount: number) => `${Math.floor(Math.abs(amount) / 100)}.${String(Math.abs(amount) % 100).padStart(2, "0")}`

const parseAmount = (value: string) => {
  const match = value.trim().match(/^(\d+)(?:\.(\d{0,2}))?$/)
  if (!match) return null
  return Number(match[1]) * 100 + Number((match[2] ?? "").padEnd(2, "0"))
}

const revealEditorChild = (scrollbox: ScrollBoxRenderable | null, childId: string) => {
  if (!scrollbox) return
  const child = scrollbox.content.findDescendantById(childId) as Renderable | undefined
  if (!child) return
  const above = child.screenY - scrollbox.viewport.screenY
  const below = child.screenY + child.height - (scrollbox.viewport.screenY + scrollbox.viewport.height)
  if (above < 0) scrollbox.scrollTop += above
  else if (below > 0) scrollbox.scrollTop += below
}

const revealScrollboxRange = (
  scrollbox: ScrollBoxRenderable | null,
  rowStart: number,
  rowHeight = 1,
) => {
  if (!scrollbox || scrollbox.viewport.height <= 0) return
  const nextScrollTop = scrollTopForRange(
    scrollbox.scrollTop,
    scrollbox.viewport.height,
    rowStart,
    rowHeight,
  )
  if (nextScrollTop !== scrollbox.scrollTop) scrollbox.scrollTo(nextScrollTop)
}

const isVirtualAccountId = (accountId: string) =>
  accountId === ALL_TRANSACTIONS_ID || accountId === UNBALANCED_TRANSACTIONS_ID

const accountRowMarginTop = (accounts: Array<{ id: string }>, index: number) =>
  index > 0 && (isVirtualAccountId(accounts[index]!.id) || isVirtualAccountId(accounts[index - 1]!.id))
    ? 1
    : 0

const accountRowTop = (accounts: Array<{ id: string }>, selectedIndex: number) => {
  let top = 1
  for (let index = 0; index <= selectedIndex; index += 1) {
    top += accountRowMarginTop(accounts, index)
    if (index < selectedIndex) top += 1
  }
  return top
}

const accountSubtree = (accounts: Account[], rootId: string) => {
  const ids = new Set([rootId])
  let changed = true
  while (changed) {
    changed = false
    for (const account of accounts) {
      if (account.parent_id && ids.has(account.parent_id) && !ids.has(account.id)) {
        ids.add(account.id)
        changed = true
      }
    }
  }
  return accounts.filter((account) => ids.has(account.id))
}

export function App() {
  const renderer = useRenderer()
  const { width: terminalWidth } = useTerminalDimensions()
  const registry = useContext(RegistryContext)
  const accountListRef = useRef<ScrollBoxRenderable>(null)
  const transactionListRef = useRef<ScrollBoxRenderable>(null)
  const transactionEditorRef = useRef<ScrollBoxRenderable>(null)
  const notesRef = useRef<TextareaRenderable>(null)
  const notesMeasurementRef = useRef<TextRenderable>(null)
  const state = useAtomValue(stateAtom)
  const selectedAccount = useAtomValue(selectedAccountAtom)
  const allTransactionsSelected = state.selectedAccountId === ALL_TRANSACTIONS_ID
  const unbalancedTransactionsSelected = state.selectedAccountId === UNBALANCED_TRANSACTIONS_ID
  const virtualAccountSelected = allTransactionsSelected || unbalancedTransactionsSelected
  const showAccountColumn = virtualAccountSelected || (selectedAccount?.has_children ?? false)
  const financialAccounts = useAtomValue(financialAccountsAtom)
  const accountView = useAtomValue(accountViewAtom)
  const transactionSearch = useAtomValue(transactionSearchAtom)
  const syncLog = useAtomValue(syncLogAtom)
  const financialAccountLayoutKey = financialAccounts.map((account) => account.id).join("\0")
  const collapsedAccountIds = useAtomValue(collapsedAccountIdsAtom)
  const selectedTransaction = useAtomValue(selectedTransactionAtom)
  const assignmentAccounts = useAtomValue(assignmentAccountsAtom)
  const creationParents = useAtomValue(creationParentsAtom)
  const [creationField, setCreationField] = useState<"name" | "parent" | "type" | "button">("name")
  const [openCreationSelect, setOpenCreationSelect] = useState<"parent" | "type" | null>(null)
  const [deletingAccount, setDeletingAccount] = useState<Account | null>(null)
  const [deletionChoice, setDeletionChoice] = useState<0 | 1>(0)
  const [screen, setScreen] = useState<"ledger" | "plugins" | "debug-sync">("ledger")
  const [transactionDraft, setTransactionDraft] = useState<TransactionDraft | null>(null)
  const [transactionEditorOpen, setTransactionEditorOpen] = useState(false)
  const [transactionEditing, setTransactionEditing] = useState(false)
  const [normalEditorFocus, setNormalEditorFocus] = useState<
    "register" | "edit" | "notes" | "counterpart" | "split" | "save" | "cancel"
  >("register")
  const [normalEditorSourceId, setNormalEditorSourceId] = useState<string | null>(null)
  const [normalNotes, setNormalNotes] = useState("")
  const [normalCounterpartAccountId, setNormalCounterpartAccountId] = useState(NO_ACCOUNT)
  const [collapseSplitsToSingle, setCollapseSplitsToSingle] = useState(false)
  const [transactionEditorFocus, setTransactionEditorFocus] = useState("notes")
  const [openTransactionAccountSelect, setOpenTransactionAccountSelect] = useState<string | null>(null)
  const [transactionEditorError, setTransactionEditorError] = useState<string | null>(null)
  const [searchDraft, setSearchDraft] = useState(transactionSearch)
  const [searchFocused, setSearchFocused] = useState(false)
  const [syncLogOpen, setSyncLogOpen] = useState(false)
  const [syncOptionsOpen, setSyncOptionsOpen] = useState(false)
  const [syncEndDate, setSyncEndDate] = useState(new Date().toISOString().slice(0, 10))
  const [syncDateError, setSyncDateError] = useState<string | null>(null)
  const syncLogRef = useRef<ScrollBoxRenderable>(null)
  const counterpartAnchorRef = useRef<BoxRenderable>(null)
  const payeeAnchorRef = useRef<BoxRenderable>(null)
  const [counterpartPickerOpen, setCounterpartPickerOpen] = useState(false)
  const [counterpartFilter, setCounterpartFilter] = useState("")
  const [counterpartHighlight, setCounterpartHighlight] = useState(0)
  const [payeePickerOpen, setPayeePickerOpen] = useState(false)
  const [payeeFilter, setPayeeFilter] = useState("")
  const [payeeHighlight, setPayeeHighlight] = useState(0)
  const [notesHeight, setNotesHeight] = useState(3)
  const notesMeasureCallback = useRef<(() => void) | null>(null)
  const showAccountSidebar = terminalWidth >= ACCOUNT_SIDEBAR_WIDTH +
    MIN_TRANSACTION_PANE_WIDTH + (transactionEditorOpen ? TRANSACTION_EDITOR_WIDTH : 0)
  const transactionTableWidth = terminalWidth -
    (showAccountSidebar ? ACCOUNT_SIDEBAR_WIDTH : 0) -
    (transactionEditorOpen ? TRANSACTION_EDITOR_WIDTH : 0) - 2
  const showTransactionAccountColumn = showAccountColumn &&
    transactionTableWidth >= TRANSACTION_TABLE_WITH_ACCOUNT_WIDTH
  const showTransactionCounterpartColumn = transactionTableWidth >=
    TRANSACTION_TABLE_WITH_COUNTERPART_WIDTH
  const newAccountParent = state.accounts.find(
    (account) => account.id === state.newAccountParentId,
  ) ?? null
  const editingAccount = state.accounts.find(
    (account) => account.id === state.editingAccountId,
  ) ?? null
  const editingSyncedAccount = editingAccount !== null && editingAccount.external_id !== null
  const unavailableParentIds = new Set(
    editingAccount
      ? accountSubtree(state.accounts, editingAccount.id).map((account) => account.id)
      : [],
  )
  const openSyncOptions = () => {
    if (state.busy || syncLog.running) return
    setSyncEndDate(new Date().toISOString().slice(0, 10))
    setSyncDateError(null)
    setSyncOptionsOpen(true)
  }
  const runConfiguredSync = () => {
    if (!/^\d{4}-\d{2}-\d{2}$/.test(syncEndDate)) {
      setSyncDateError("Use YYYY-MM-DD")
      return
    }
    setSyncOptionsOpen(false)
    sync(registry, { endDate: syncEndDate })
  }
  const openCounterpartPicker = () => {
    if (
      !selectedTransaction ||
      state.busy ||
      transactionEditorOpen ||
      selectedTransaction.other_postings.length > 1 ||
      !showTransactionCounterpartColumn
    ) return
    const current = selectedTransaction.other_postings[0]?.account_id
    setCounterpartFilter("")
    setCounterpartHighlight(Math.max(
      0,
      assignmentAccounts.findIndex((account) => account.id === current),
    ))
    setPayeePickerOpen(false)
    setCounterpartPickerOpen(true)
  }
  const selectInlineCounterpart = (accountId: string) => {
    setCounterpartPickerOpen(false)
    setInlineCounterpart(registry, accountId)
  }
  const openPayeePicker = () => {
    if (!selectedTransaction || state.busy || transactionEditorOpen) return
    setPayeeFilter("")
    setPayeeHighlight(0)
    setCounterpartPickerOpen(false)
    setPayeePickerOpen(true)
  }
  const selectInlinePayee = (payee: string) => {
    setPayeePickerOpen(false)
    setInlinePayee(registry, payee)
  }
  const assignmentTree = assignmentAccounts
  const selectedAssignment = assignmentAccounts[state.selectedAssignmentIndex] ?? null
  const normalizedCounterpartFilter = counterpartFilter.trim().toLocaleLowerCase()
  const counterpartOptions = assignmentAccounts.filter((account) =>
    !normalizedCounterpartFilter ||
    `${account.name} ${account.type}`.toLocaleLowerCase().includes(normalizedCounterpartFilter)
  )
  const counterpartVisibleCount = Math.min(10, Math.max(1, counterpartOptions.length))
  const counterpartVisibleStart = Math.max(
    0,
    Math.min(
      counterpartHighlight - Math.floor(counterpartVisibleCount / 2),
      counterpartOptions.length - counterpartVisibleCount,
    ),
  )
  const visibleCounterpartOptions = counterpartOptions.slice(
    counterpartVisibleStart,
    counterpartVisibleStart + counterpartVisibleCount,
  )
  const normalizedPayeeFilter = payeeFilter.trim().toLocaleLowerCase()
  const payeeOptions = normalizedPayeeFilter
    ? state.payees.filter((payee) => payee.toLocaleLowerCase().startsWith(normalizedPayeeFilter))
    : []
  const payeeVisibleCount = Math.min(10, Math.max(1, payeeOptions.length))
  const payeeVisibleStart = Math.max(
    0,
    Math.min(
      payeeHighlight - Math.floor(payeeVisibleCount / 2),
      payeeOptions.length - payeeVisibleCount,
    ),
  )
  const visiblePayeeOptions = payeeOptions.slice(
    payeeVisibleStart,
    payeeVisibleStart + payeeVisibleCount,
  )
  const editingPosting = selectedTransaction?.other_postings.find(
    (posting) => posting.posting_id === state.editingPostingId,
  )
  const assignmentAmount = editingPosting?.amount ?? selectedTransaction?.unallocated_amount ?? 0
  const deletionAccounts = deletingAccount
    ? accountSubtree(state.accounts, deletingAccount.id)
    : []
  const deletionContainsSyncedAccount = deletionAccounts.some(
    (account) => account.external_id !== null,
  )
  const deletingCategory = deletingAccount?.type === "income" || deletingAccount?.type === "expense"
  const registerVisible = !deletingAccount && !state.creatingAccount && !state.assigning
  const showRegisterBalance = registerVisible && selectedAccount !== null && !virtualAccountSelected
  const parentOptions: FilterSelectOption<string>[] = [
    { value: NO_PARENT, label: "No parent", detail: editingAccount ? "top level" : "new root" },
    ...creationParents.filter((account) => !unavailableParentIds.has(account.id)).map((account) => ({
      value: account.id,
      label: account.name,
      detail: account.type,
      searchText: `${account.name} ${account.type}`,
      group: account.type,
      depth: account.depth,
    })),
  ]
  const typeOptions: FilterSelectOption<Account["type"]>[] = accountTypes.map((accountType) => ({
    value: accountType,
    label: accountType,
  }))
  const selectedPostings = selectedTransaction
    ? [
        {
          postingId: selectedTransaction.posting_id,
          accountId: selectedTransaction.account_id,
          amount: selectedTransaction.amount,
        },
        ...selectedTransaction.other_postings.map((posting) => ({
          postingId: posting.posting_id,
          accountId: posting.account_id,
          amount: posting.amount,
        })),
      ]
    : []
  const selectedCanonicalPosting = selectedTransaction
    ? selectedPostings.find((posting) => posting.postingId === selectedTransaction.source_id) ?? null
    : null
  const selectedViewPosting = selectedTransaction
    ? selectedPostings.find((posting) => posting.postingId === selectedTransaction.posting_id) ?? null
    : null
  const selectedCounterparts = selectedTransaction
    ? selectedPostings.filter((posting) => posting.postingId !== selectedTransaction.posting_id)
    : []
  const splitCounterpartAmountWidth = selectedCounterparts.reduce(
    (width, posting) => Math.max(width, stringWidth(money(posting.amount))),
    0,
  )
  const showSplitSummary = selectedCounterparts.length > 1 && !collapseSplitsToSingle
  const normalEditorFields: Array<typeof normalEditorFocus> = transactionEditing
    ? showSplitSummary
      ? ["notes", "split", "save", "cancel"]
      : ["notes", "counterpart", "split", "save", "cancel"]
    : ["register", "edit"]
  const editorSourceAccountId = transactionDraft?.sourceAccountId ?? selectedViewPosting?.accountId ?? null
  const editorSourceAmount = transactionDraft?.sourceAmount ?? selectedViewPosting?.amount ?? 0
  const transactionEditorFields = transactionDraft
    ? [
        "notes",
        ...transactionDraft.allocations.flatMap((allocation) => [
          `account:${allocation.postingId}`,
          `amount:${allocation.postingId}`,
        ]),
        "add",
        "save",
        "cancel",
      ]
    : []
  const parsedAllocationAmounts = transactionDraft?.allocations.map((allocation) => parseAmount(allocation.amount)) ?? []
  const remainingAmount = transactionDraft
    ? Math.abs(transactionDraft.sourceAmount) - parsedAllocationAmounts.reduce<number>(
        (sum, amount) => sum + (amount ?? 0),
        0,
      )
    : 0
  const transactionCanSave = transactionDraft !== null &&
    remainingAmount === 0 &&
    parsedAllocationAmounts.every((amount) => amount !== null)

  const loadNormalTransaction = (transaction: typeof selectedTransaction) => {
    if (!transaction) return
    const postings = [
      { postingId: transaction.posting_id, accountId: transaction.account_id },
      ...transaction.other_postings.map((posting) => ({
        postingId: posting.posting_id,
        accountId: posting.account_id,
      })),
    ]
    const counterparts = postings.filter((posting) => posting.postingId !== transaction.posting_id)
    setNormalEditorSourceId(transaction.source_id)
    setNormalNotes(transaction.notes ?? "")
    setNormalCounterpartAccountId(
      counterparts.length === 1 ? counterparts[0]?.accountId ?? NO_ACCOUNT : NO_ACCOUNT,
    )
    setCollapseSplitsToSingle(false)
    setNotesHeight(3)
  }

  const scheduleNotesHeightMeasurement = (moveCursorToEnd = false) => {
    if (notesMeasureCallback.current) renderer.off("frame", notesMeasureCallback.current)
    const measure = () => {
      notesMeasureCallback.current = null
      const textarea = notesRef.current
      if (!textarea) return
      setNotesHeight(Math.max(3, notesMeasurementRef.current?.virtualLineCount ?? 1))
      if (moveCursorToEnd) textarea.gotoBufferEnd()
    }
    notesMeasureCallback.current = measure
    renderer.once("frame", measure)
  }

  const openTransactionEditor = () => {
    if (!selectedTransaction) return
    setTransactionEditorOpen(true)
    setTransactionEditing(false)
    setNormalEditorFocus("register")
    loadNormalTransaction(selectedTransaction)
  }

  const beginTransactionEditMode = () => {
    if (!selectedTransaction) return
    setTransactionEditing(true)
    setNormalEditorFocus("notes")
    loadNormalTransaction(selectedTransaction)
    if (selectedCounterparts.length > 1) beginTransactionEditing()
  }

  const cancelTransactionEditMode = () => {
    loadNormalTransaction(selectedTransaction)
    setTransactionDraft(null)
    setOpenTransactionAccountSelect(null)
    setTransactionEditorError(null)
    setTransactionEditing(false)
    setNormalEditorFocus("edit")
  }

  const saveTransactionEditMode = () => {
    if (!selectedTransaction || !selectedCanonicalPosting || !selectedViewPosting) return
    const changes: TransactionChange[] = [{
      operation: "set" as const,
      target_id: selectedTransaction.source_id,
      field: "notes" as const,
      value: normalNotes.trim() || null,
    }]
    const originalCounterpartAccountId = selectedCounterparts.length === 1
      ? selectedCounterparts[0]?.accountId ?? NO_ACCOUNT
      : NO_ACCOUNT
    if (
      (selectedCounterparts.length <= 1 || collapseSplitsToSingle) &&
      (collapseSplitsToSingle || normalCounterpartAccountId !== originalCounterpartAccountId)
    ) {
      if (selectedTransaction.posting_id === selectedTransaction.source_id) {
        changes.push(...selectedCounterparts.map((posting) => ({
          operation: "remove_split" as const,
          target_id: posting.postingId,
        })))
        if (normalCounterpartAccountId !== NO_ACCOUNT) {
          const postingId = crypto.randomUUID()
          changes.push(
            { operation: "split" as const, target_id: postingId },
            {
              operation: "set" as const,
              target_id: postingId,
              field: "account_id" as const,
              value: normalCounterpartAccountId,
            },
            {
              operation: "set" as const,
              target_id: postingId,
              field: "amount" as const,
              value: -selectedCanonicalPosting.amount,
            },
          )
        }
      } else if (
        selectedCounterparts.length === 1 &&
        selectedCounterparts[0]?.postingId === selectedTransaction.source_id
      ) {
        changes.push({
          operation: "set" as const,
          target_id: selectedTransaction.source_id,
          field: "account_id" as const,
          value: normalCounterpartAccountId === NO_ACCOUNT ? null : normalCounterpartAccountId,
        })
      }
    }
    saveTransactionChanges(registry, selectedTransaction.source_id, changes, () => {
      setCollapseSplitsToSingle(false)
      setTransactionEditing(false)
      setNormalEditorFocus("edit")
    })
  }

  const closeTransactionEditor = () => {
    setTransactionEditorOpen(false)
    setTransactionEditing(false)
    setTransactionDraft(null)
    setCollapseSplitsToSingle(false)
    setOpenTransactionAccountSelect(null)
  }

  const moveTransactionViewport = (direction: -1 | 1) => {
    const scrollbox = transactionListRef.current
    if (!scrollbox || scrollbox.viewport.height <= 2) return
    const viewportHeight = scrollbox.viewport.height
    const viewportStart = Math.round(scrollbox.scrollTop)
    const step = viewportHeight - 2
    if (direction === -1 && viewportStart === 0) return
    const nextViewportStart = Math.max(0, viewportStart + direction * step)
    const selectedIndex = direction === 1
      ? nextViewportStart
      : nextViewportStart + viewportHeight - 1
    selectBufferedTransaction(registry, selectedIndex)
    scrollbox.scrollTo(nextViewportStart)
  }

  const beginTransactionEditing = () => {
    if (!selectedTransaction || !selectedCanonicalPosting) return
    const currentPosting = {
      postingId: selectedTransaction.posting_id,
      accountId: selectedTransaction.account_id,
      amount: selectedTransaction.amount,
    }
    const otherPostings = selectedTransaction.other_postings.map((posting) => ({
      postingId: posting.posting_id,
      accountId: posting.account_id,
      amount: posting.amount,
    }))
    const postings = [currentPosting, ...otherPostings]
    const sourcePosting = selectedCanonicalPosting
    let allocations: AllocationDraft[] = postings
      .filter((posting) => posting.postingId !== selectedTransaction.source_id)
      .map((posting) => ({
        postingId: posting.postingId,
        accountId: posting.accountId,
        amount: decimalAmount(posting.amount),
        existing: true,
      }))
    const originalPostingIds = allocations.map((allocation) => allocation.postingId)
    if (collapseSplitsToSingle) {
      allocations = [{
        postingId: crypto.randomUUID(),
        accountId: normalCounterpartAccountId === NO_ACCOUNT ? null : normalCounterpartAccountId,
        amount: "",
        existing: false,
      }]
    }
    if (allocations.length <= 1 && normalCounterpartAccountId !== (
      allocations[0]?.accountId ?? NO_ACCOUNT
    )) {
      allocations = [{
        postingId: crypto.randomUUID(),
        accountId: normalCounterpartAccountId === NO_ACCOUNT ? null : normalCounterpartAccountId,
        amount: "",
        existing: false,
      }]
    }
    if (allocations.length === 0) {
      allocations.push({
        postingId: crypto.randomUUID(),
        accountId: null,
        amount: "",
        existing: false,
      })
    }
    while (allocations.length < 2) {
      allocations.push({
        postingId: crypto.randomUUID(),
        accountId: null,
        amount: "",
        existing: false,
      })
    }
    setTransactionDraft({
      sourceId: selectedTransaction.source_id,
      sourceAccountId: sourcePosting.accountId,
      sourceAmount: sourcePosting.amount,
      originalPostingIds,
      allocations,
    })
    setCollapseSplitsToSingle(false)
    setTransactionEditorFocus(`account:${allocations[0]?.postingId}`)
    setOpenTransactionAccountSelect(null)
    setTransactionEditorError(null)
  }

  const closeTransactionEditing = () => {
    setTransactionDraft(null)
    setOpenTransactionAccountSelect(null)
    setTransactionEditorError(null)
  }

  const addAllocation = () => {
    if (!transactionDraft) return
    const postingId = crypto.randomUUID()
    setTransactionDraft((draft) => draft ? {
      ...draft,
      allocations: [...draft.allocations, {
        postingId,
        accountId: null,
        amount: "",
        existing: false,
      }],
    } : null)
    setTransactionEditorFocus(`account:${postingId}`)
  }

  const removeAllocation = (postingId: string) => {
    if (!transactionDraft) return
    const allocations = transactionDraft.allocations.filter(
      (allocation) => allocation.postingId !== postingId,
    )
    if (allocations.length === 1) {
      setNormalCounterpartAccountId(allocations[0]?.accountId ?? NO_ACCOUNT)
      setCollapseSplitsToSingle(true)
      setTransactionDraft(null)
      setOpenTransactionAccountSelect(null)
      setNormalEditorFocus("counterpart")
    } else {
      setTransactionDraft({ ...transactionDraft, allocations })
      setTransactionEditorFocus("add")
    }
    setTransactionEditorError(null)
  }

  const saveTransaction = () => {
    if (!transactionDraft) return
    if (remainingAmount !== 0) {
      setTransactionEditorError("Allocations must balance before saving")
      return
    }
    if (parsedAllocationAmounts.some((amount) => amount === null)) {
      setTransactionEditorError("Enter a valid amount for every split")
      return
    }
    const direction = transactionDraft.sourceAmount < 0 ? 1 : -1
    const activeAllocations = transactionDraft.allocations.filter(
      (_, index) => parsedAllocationAmounts[index] !== 0,
    )
    const currentIds = new Set(activeAllocations.map((allocation) => allocation.postingId))
    const changes = [
      {
        operation: "set" as const,
        target_id: transactionDraft.sourceId,
        field: "notes" as const,
        value: normalNotes.trim() || null,
      },
      ...transactionDraft.originalPostingIds
        .filter((postingId) => !currentIds.has(postingId))
        .map((postingId) => ({ operation: "remove_split" as const, target_id: postingId })),
      ...activeAllocations.flatMap((allocation) => {
        const index = transactionDraft.allocations.findIndex((candidate) => candidate.postingId === allocation.postingId)
        return [
        ...(allocation.existing ? [] : [{ operation: "split" as const, target_id: allocation.postingId }]),
        {
          operation: "set" as const,
          target_id: allocation.postingId,
          field: "account_id" as const,
          value: allocation.accountId,
        },
        {
          operation: "set" as const,
          target_id: allocation.postingId,
          field: "amount" as const,
          value: parsedAllocationAmounts[index]! * direction,
        },
      ]}),
    ]
    saveTransactionChanges(registry, transactionDraft.sourceId, changes, () => {
      closeTransactionEditing()
      setCollapseSplitsToSingle(false)
      setTransactionEditing(false)
      setNormalEditorFocus("edit")
    })
  }

  useEffect(() => {
    initialize(registry)
  }, [registry])

  useEffect(() => {
    if (!searchFocused) setSearchDraft(transactionSearch)
  }, [searchFocused, transactionSearch])

  useEffect(() => {
    if (searchDraft === transactionSearch) return
    const timeout = setTimeout(() => setTransactionSearch(registry, searchDraft), 200)
    return () => clearTimeout(timeout)
  }, [registry, searchDraft, transactionSearch])

  useEffect(() => {
    if (!state.creatingAccount) return
    setCreationField("name")
    setOpenCreationSelect(null)
  }, [state.creatingAccount])

  useEffect(() => {
    if (showAccountSidebar || state.focus !== "accounts") return
    registry.update(stateAtom, (current) => ({ ...current, focus: "transactions" as const }))
  }, [registry, showAccountSidebar, state.focus])

  useEffect(() => {
    if (syncLogOpen || syncOptionsOpen || deletingAccount || state.creatingAccount || state.assigning || transactionEditing || transactionDraft) {
      accountListRef.current?.blur()
      transactionListRef.current?.blur()
      return
    }
    if (transactionEditorOpen) {
      if (normalEditorFocus === "register") transactionListRef.current?.focus()
      else {
        accountListRef.current?.blur()
        transactionListRef.current?.blur()
      }
      return
    }
    if (state.focus === "accounts") accountListRef.current?.focus()
    else transactionListRef.current?.focus()
  }, [
    deletingAccount,
    normalEditorFocus,
    state.assigning,
    state.creatingAccount,
    state.focus,
    syncLogOpen,
    syncOptionsOpen,
    transactionDraft,
    transactionEditing,
    transactionEditorOpen,
  ])

  useEffect(() => {
    if (!syncLogOpen) return
    syncLogRef.current?.scrollTo(Math.max(0, syncLog.lines.length - 1))
  }, [syncLog.lines.length, syncLogOpen])

  useEffect(() => {
    setCounterpartPickerOpen(false)
    setPayeePickerOpen(false)
  }, [selectedTransaction?.posting_id])

  useEffect(() => {
    setCounterpartHighlight((index) => Math.min(index, Math.max(0, counterpartOptions.length - 1)))
  }, [counterpartOptions.length, counterpartFilter])

  useEffect(() => {
    setPayeeHighlight((index) => Math.min(index, Math.max(0, payeeOptions.length - 1)))
  }, [payeeOptions.length, payeeFilter])

  useEffect(() => {
    if (!counterpartPickerOpen && !payeePickerOpen) return
    const initialScrollTop = transactionListRef.current?.scrollTop
    const closeOnScroll = () => {
      if (transactionListRef.current?.scrollTop !== initialScrollTop) {
        setCounterpartPickerOpen(false)
        setPayeePickerOpen(false)
      }
    }
    renderer.on("frame", closeOnScroll)
    return () => {
      renderer.off("frame", closeOnScroll)
    }
  }, [counterpartPickerOpen, payeePickerOpen, renderer])

  useEffect(() => {
    if (!state.trace) return
    recordPerformance("frontend.render_commit", {
      trace_id: state.trace.id,
      source_event: state.trace.event,
      rows: state.trace.rows,
      elapsed_ms: performance.now() - state.trace.startedAt,
    })
  }, [state.trace])

  useEffect(() => {
    const selectedIndex = financialAccounts.findIndex((account) => account.id === state.selectedAccountId)
    if (selectedIndex < 0) return
    const reveal = () => {
      revealScrollboxRange(accountListRef.current, accountRowTop(financialAccounts, selectedIndex))
    }
    renderer.once("frame", reveal)
    return () => {
      renderer.off("frame", reveal)
    }
  }, [financialAccountLayoutKey, renderer, state.selectedAccountId])

  useEffect(() => {
    const transaction = state.transactions[state.selectedTransactionIndex]
    if (!transaction) return
    const selectedIndex = state.selectedTransactionIndex
    const reveal = () => {
      revealScrollboxRange(transactionListRef.current, selectedIndex)
    }
    renderer.once("frame", reveal)
    return () => {
      renderer.off("frame", reveal)
    }
  }, [renderer, state.selectedAccountId, state.selectedTransactionIndex, selectedTransaction?.source_id])

  useEffect(() => {
    revealEditorChild(transactionEditorRef.current, `transaction-editor-${transactionEditorFocus}`)
  }, [transactionEditorFocus, transactionDraft?.allocations.length])

  useEffect(() => {
    if (!transactionEditorOpen || transactionEditing || transactionDraft || normalEditorFocus !== "edit") return
    revealEditorChild(transactionEditorRef.current, "transaction-editor-edit")
  }, [normalEditorFocus, transactionDraft, transactionEditing, transactionEditorOpen])

  useEffect(() => {
    if (!transactionEditorOpen || !transactionEditing || !notesRef.current) return
    scheduleNotesHeightMeasurement(true)
  }, [normalEditorSourceId, transactionDraft, transactionEditing, transactionEditorOpen])

  useEffect(() => {
    if (
      !transactionEditorOpen ||
      transactionDraft ||
      !selectedTransaction ||
      selectedTransaction.source_id === normalEditorSourceId
    ) return
    loadNormalTransaction(selectedTransaction)
  }, [selectedTransaction?.source_id, transactionDraft, transactionEditorOpen])

  useEffect(() => {
    if (
      transactionEditorOpen &&
      transactionEditing &&
      !transactionDraft &&
      showSplitSummary &&
      normalEditorFocus === "counterpart"
    ) setNormalEditorFocus("split")
  }, [normalEditorFocus, showSplitSummary, transactionDraft, transactionEditing, transactionEditorOpen])

  useEffect(() => () => {
    if (notesMeasureCallback.current) renderer.off("frame", notesMeasureCallback.current)
  }, [])

  useKeyboard((key) => {
    const tabPressed = key.name === "tab" || key.name === "shift-tab"
    const reverseTab = key.shift || key.name === "shift-tab"
    if (syncOptionsOpen) {
      if (key.name === "escape") {
        key.preventDefault()
        key.stopPropagation()
        setSyncOptionsOpen(false)
      }
      return
    }
    if (syncLogOpen) {
      if (key.name === "escape" || key.name === "return" || key.name === "enter") {
        key.preventDefault()
        key.stopPropagation()
        setSyncLogOpen(false)
      }
      return
    }
    if (payeePickerOpen) {
      if (["escape", "up", "down", "return", "enter"].includes(key.name)) {
        key.preventDefault()
        key.stopPropagation()
      }
      if (key.name === "escape") setPayeePickerOpen(false)
      if (key.name === "up" && payeeOptions.length > 0) {
        setPayeeHighlight((index) => Math.max(0, index - 1))
      }
      if (key.name === "down" && payeeOptions.length > 0) {
        setPayeeHighlight((index) => Math.min(payeeOptions.length - 1, index + 1))
      }
      if (key.name === "return" || key.name === "enter") {
        const payee = payeeOptions[payeeHighlight] ?? payeeFilter.trim()
        if (payee) selectInlinePayee(payee)
      }
      return
    }
    if (counterpartPickerOpen) {
      if (["escape", "up", "down", "return", "enter"].includes(key.name)) {
        key.preventDefault()
        key.stopPropagation()
      }
      if (key.name === "escape") setCounterpartPickerOpen(false)
      if (key.name === "up" && counterpartOptions.length > 0) {
        setCounterpartHighlight((index) => Math.max(0, index - 1))
      }
      if (key.name === "down" && counterpartOptions.length > 0) {
        setCounterpartHighlight((index) => Math.min(counterpartOptions.length - 1, index + 1))
      }
      if (key.name === "return" || key.name === "enter") {
        const account = counterpartOptions[counterpartHighlight]
        if (account) selectInlineCounterpart(account.id)
      }
      return
    }
    if (
      key.name === "s" &&
      !transactionEditing &&
      !transactionDraft &&
      !state.creatingAccount &&
      !state.assigning &&
      !deletingAccount
    ) {
      openSyncOptions()
      return
    }
    if (screen !== "ledger") return
    if (searchFocused) {
      if (key.name === "escape" || key.name === "return" || key.name === "enter") {
        setSearchFocused(false)
      }
      return
    }
    if (key.name === "c" && state.focus === "transactions") {
      openCounterpartPicker()
      return
    }
    if (key.name === "p" && state.focus === "transactions") {
      openPayeePicker()
      return
    }
    if (transactionDraft) {
      if (openTransactionAccountSelect) return
      if (key.name === "escape") cancelTransactionEditMode()
      if (key.ctrl && key.name === "s") {
        key.preventDefault()
        if (transactionCanSave) saveTransaction()
      }
      if (tabPressed || (key.name === "down" && transactionEditorFocus !== "notes")) {
        if (tabPressed) {
          key.preventDefault()
          key.stopPropagation()
        }
        if (key.name === "down") {
          key.preventDefault()
          key.stopPropagation()
        }
        setTransactionEditorFocus(moveOrderedFocus(
          transactionEditorFields,
          transactionEditorFocus,
          tabPressed && reverseTab ? -1 : 1,
        ))
      }
      if (key.name === "up" && transactionEditorFocus !== "notes") {
        key.preventDefault()
        key.stopPropagation()
        const current = transactionEditorFields.indexOf(transactionEditorFocus)
        setTransactionEditorFocus(
          transactionEditorFields[(current - 1 + transactionEditorFields.length) % transactionEditorFields.length],
        )
      }
      if ((key.name === "return" || key.name === "enter") && transactionEditorFocus === "add") {
        addAllocation()
      }
      if ((key.name === "return" || key.name === "enter") && transactionEditorFocus === "save") {
        if (transactionCanSave) saveTransaction()
      }
      if ((key.name === "return" || key.name === "enter") && transactionEditorFocus === "cancel") {
        cancelTransactionEditMode()
      }
      if (key.name === "d" && transactionEditorFocus.includes(":")) {
        const postingId = transactionEditorFocus.split(":")[1]
        removeAllocation(postingId)
      }
      return
    }
    if (transactionEditorOpen) {
      if (openTransactionAccountSelect) return
      if (key.name === "escape") {
        if (transactionEditing) {
          cancelTransactionEditMode()
        } else closeTransactionEditor()
      }
      if (tabPressed) {
        key.preventDefault()
        key.stopPropagation()
        setNormalEditorFocus((focus) => moveOrderedFocus(normalEditorFields, focus, reverseTab ? -1 : 1))
      }
      if (
        !transactionEditing &&
        normalEditorFocus === "register" &&
        (key.name === "up" || key.name === "k" || key.name === "down" || key.name === "j")
      ) {
        key.preventDefault()
        key.stopPropagation()
        moveSelection(registry, key.name === "up" || key.name === "k" ? -1 : 1)
      }
      if (
        !transactionEditing &&
        normalEditorFocus === "register" &&
        (key.name === "pageDown" || key.name === "pagedown" ||
          key.name === "pageUp" || key.name === "pageup")
      ) {
        key.preventDefault()
        key.stopPropagation()
        moveTransactionViewport(
          key.name === "pageUp" || key.name === "pageup" ? -1 : 1,
        )
      }
      if (
        transactionEditing &&
        normalEditorFocus === "split" &&
        (key.name === "return" || key.name === "enter")
      ) beginTransactionEditing()
      if (
        transactionEditing &&
        normalEditorFocus === "save" &&
        (key.name === "return" || key.name === "enter")
      ) saveTransactionEditMode()
      if (
        transactionEditing &&
        normalEditorFocus === "cancel" &&
        (key.name === "return" || key.name === "enter")
      ) cancelTransactionEditMode()
      if (
        !transactionEditing &&
        normalEditorFocus === "edit" &&
        (key.name === "return" || key.name === "enter")
      ) beginTransactionEditMode()
      return
    }
    if (deletingAccount) {
      if (key.name === "escape") setDeletingAccount(null)
      if (!deletionContainsSyncedAccount) {
        if (
          !deletingCategory &&
          (key.name === "up" || key.name === "down" || key.name === "k" || key.name === "j")
        ) {
          setDeletionChoice((choice) => choice === 0 ? 1 : 0)
        }
        if (key.name === "return" || key.name === "enter") {
          deleteAccount(registry, deletingAccount.id, !deletingCategory && deletionChoice === 1)
          setDeletingAccount(null)
        }
      }
      return
    }
    if (state.creatingAccount) {
      if (openCreationSelect) return
      if (key.name === "escape") cancelAccountCreation(registry)
      if (tabPressed) {
        key.preventDefault()
        key.stopPropagation()
        const fields: Array<typeof creationField> = editingSyncedAccount
          ? ["name", "button"]
          : state.newAccountParentId === null
            ? ["name", "parent", "type", "button"]
            : ["name", "parent", "button"]
        setCreationField((current) => moveOrderedFocus(fields, current, reverseTab ? -1 : 1))
      }
      if ((key.name === "return" || key.name === "enter") && creationField === "button") {
        createAccount(registry)
      }
      return
    }
    if (key.name === "q") renderer.destroy()
    if (key.name === "escape") {
      if (state.assigning) cancelAssignment(registry)
      else renderer.destroy()
    }
    if (key.name === "up" || key.name === "k") {
      key.preventDefault()
      key.stopPropagation()
      moveSelection(registry, -1)
    }
    if (key.name === "down" || key.name === "j") {
      key.preventDefault()
      key.stopPropagation()
      moveSelection(registry, 1)
    }
    if (
      (key.name === "left" || key.name === "right") &&
      state.focus === "accounts" &&
      selectedAccount?.has_children
    ) {
      key.preventDefault()
      key.stopPropagation()
      setAccountCollapsed(registry, selectedAccount.id, key.name === "left")
    }
    if (tabPressed) {
      key.preventDefault()
      key.stopPropagation()
      if (showAccountSidebar) toggleFocus(registry)
    }
    if (key.name === "a") beginAccountCreation(registry)
    if (key.name === "e" && state.focus === "accounts" && !virtualAccountSelected) {
      beginAccountEditing(registry)
    }
    if (key.name === "e" && state.focus === "transactions") openTransactionEditor()
    if (key.name === "d" && state.focus === "accounts" && selectedAccount && !virtualAccountSelected) {
      setDeletingAccount(selectedAccount)
      setDeletionChoice(0)
    }
    if (
      state.focus === "transactions" &&
      (key.name === "pageDown" || key.name === "pagedown" ||
        key.name === "pageUp" || key.name === "pageup")
    ) {
      key.preventDefault()
      key.stopPropagation()
      moveTransactionViewport(
        key.name === "pageUp" || key.name === "pageup" ? -1 : 1,
      )
    }
    if (key.name === "return" || key.name === "enter") {
      if (state.assigning) {
        if (state.choosingPosting) choosePosting(registry)
        else assignToAccount(registry)
      }
      else if (state.focus === "transactions") openTransactionEditor()
    }
    if (key.name === "r") refresh(registry)
  })

  const notesFieldFocused = transactionDraft
    ? transactionEditorFocus === "notes"
    : normalEditorFocus === "notes"
  const editableNotesField = (
    <FormField
      id="transaction-editor-notes"
      label="Notes"
      focused={notesFieldFocused}
      contentHeight={notesHeight}
    >
      <text
        ref={notesMeasurementRef}
        fg={gray[100]}
        wrapMode="word"
        style={{
          position: "absolute",
          width: TRANSACTION_EDITOR_TEXT_WIDTH,
          opacity: 0,
        }}
      >
        {normalNotes || " "}
      </text>
      <box style={{ width: "100%", height: notesHeight }}>
        <textarea
          key={normalEditorSourceId ?? "transaction-notes"}
          ref={notesRef}
          focused={notesFieldFocused}
          initialValue={normalNotes}
          placeholder="No notes"
          wrapMode="word"
          textColor={gray[100]}
          focusedTextColor={gray[100]}
          backgroundColor={gray[800]}
          focusedBackgroundColor={gray[700]}
          cursorColor={gray[200]}
          style={{
            width: TRANSACTION_EDITOR_CONTENT_WIDTH,
            height: "100%",
            paddingLeft: 1,
            paddingRight: 1,
          }}
          onMouseDown={() => {
            if (transactionDraft) setTransactionEditorFocus("notes")
            else setNormalEditorFocus("notes")
          }}
          onContentChange={() => {
            const textarea = notesRef.current
            if (!textarea || !normalEditorSourceId) return
            const notes = textarea.plainText
            scheduleNotesHeightMeasurement()
            if (notes === normalNotes) return
            setNormalNotes(notes)
          }}
        />
      </box>
    </FormField>
  )

  return (
    <PopoverProvider>
    <box
      style={{
        width: "100%",
        height: "100%",
        flexDirection: "column",
        backgroundColor: gray[900],
      }}
    >
      <box
        style={{
          height: 1,
          flexDirection: "row",
          justifyContent: "space-between",
          backgroundColor: gray[800],
        }}
      >
        <box style={{ height: 1, flexDirection: "row" }}>
          {([
            ["assets-liabilities", "Assets & Liabilities"],
            ["income-expenses", "Income & Expenses"],
          ] as const satisfies ReadonlyArray<readonly [AccountView, string]>).map(([value, label]) => {
            const selected = screen === "ledger" && accountView === value
            return (
              <box
                key={value}
                id={`account-view-${value}`}
                style={{
                  height: 1,
                  paddingLeft: 1,
                  paddingRight: 1,
                  backgroundColor: selected ? selectionBackground : gray[800],
                }}
                onMouseDown={transactionEditing || transactionDraft
                  ? undefined
                  : () => {
                      setScreen("ledger")
                      setAccountView(registry, value)
                    }}
              >
                <text fg={selected ? gray[100] : gray[500]}>{label}</text>
              </box>
            )
          })}
          <box
            id="screen-plugins"
            style={{
              height: 1,
              paddingLeft: 1,
              paddingRight: 1,
              backgroundColor: screen === "plugins" ? selectionBackground : gray[800],
            }}
            onMouseDown={transactionEditing || transactionDraft ? undefined : () => setScreen("plugins")}
          >
            <text fg={screen === "plugins" ? gray[100] : gray[500]}>Plugins</text>
          </box>
          <box
            id="screen-debug-sync"
            style={{
              height: 1,
              paddingLeft: 1,
              paddingRight: 1,
              backgroundColor: screen === "debug-sync" ? selectionBackground : gray[800],
            }}
            onMouseDown={transactionEditing || transactionDraft ? undefined : () => setScreen("debug-sync")}
          >
            <text fg={screen === "debug-sync" ? gray[100] : gray[500]}>Debug Sync</text>
          </box>
        </box>
        <box style={{ flexDirection: "row", gap: 2 }}>
          <text
            id="sync-status"
            fg={state.error ? gray[300] : state.busy || syncLog.running ? gray[400] : gray[500]}
            onMouseDown={syncLog.lines.length > 0 ? () => setSyncLogOpen(true) : undefined}
          >
            {state.error ?? (syncLog.running ? "Syncing in background" : state.message)}
          </text>
          <text id="sync-all" fg={state.busy || syncLog.running ? gray[600] : gray[200]} onMouseDown={transactionEditing || transactionDraft || state.busy || syncLog.running ? undefined : openSyncOptions}>
            [s] sync
          </text>
        </box>
      </box>

      <box style={{ flexGrow: 1, flexDirection: "row-reverse" }}>
        {screen === "plugins" ? (
          <PluginsSettings />
        ) : screen === "debug-sync" ? (
          <DebugSync onSync={openSyncOptions} />
        ) : (
        <>
        {transactionEditorOpen ? (
          <box
            title=" Edit transaction "
            style={{
               width: TRANSACTION_EDITOR_WIDTH,
              height: "100%",
              border: ["left"],
              borderColor: gray[700],
              focusedBorderColor: gray[700],
              paddingTop: 1,
              paddingLeft: transactionEditing || transactionDraft ? 0 : 1,
              paddingRight: 1,
              flexDirection: "column",
              gap: 1,
            }}
          >
            <box
              style={{
                height: 2,
                paddingLeft: transactionEditing || transactionDraft ? 1 : 0,
                flexDirection: "column",
              }}
            >
              <text fg={gray[100]} wrapMode="none">
                <strong>{selectedTransaction?.payee || "(no payee)"}</strong>
              </text>
              <AccountAmountRow
                name={state.accounts.find((account) => account.id === editorSourceAccountId)?.name ?? "None"}
                amount={money(editorSourceAmount)}
                amountWidth={stringWidth(money(editorSourceAmount))}
                muted
              />
            </box>

            <scrollbox
              ref={transactionEditorRef}
              scrollX={false}
              scrollY
              style={{
                flexGrow: 1,
                flexDirection: "column",
                verticalScrollbarOptions: { visible: false },
              }}
            >
              {transactionDraft ? (
                <box
                  key="transaction-split-edit"
                  style={{ width: "100%", flexShrink: 0, flexDirection: "column" }}
                >
                  {editableNotesField}
                  <box style={{ height: 1 }} />
                  {transactionDraft.allocations.map((allocation, allocationIndex) => {
                    const accountField = `account:${allocation.postingId}`
                    const amountField = `amount:${allocation.postingId}`
                    const accountOpen = openTransactionAccountSelect === allocation.postingId
                    const selectedElsewhere = new Set(
                      transactionDraft.allocations
                        .filter((candidate) => candidate.postingId !== allocation.postingId)
                        .filter((candidate) => parseAmount(candidate.amount) !== 0)
                        .map((candidate) => candidate.accountId),
                    )
                    const options: FilterSelectOption<string>[] = creationParents
                      .filter((account) => account.id !== transactionDraft.sourceAccountId && !selectedElsewhere.has(account.id))
                      .map((account) => ({
                        value: account.id,
                        label: account.name,
                        detail: account.type,
                        searchText: `${account.name} ${account.type}`,
                        group: account.type,
                        depth: account.depth,
                      }))
                    return (
                      <box
                        key={allocation.postingId}
                        style={{
                          height: allocationIndex < transactionDraft.allocations.length - 1 ? 4 : 3,
                          marginLeft: 1,
                          flexDirection: "column",
                          zIndex: accountOpen ? 100 : 0,
                        }}
                        >
                        <box style={{ width: "100%", height: 1, flexDirection: "row", justifyContent: "space-between" }}>
                          <text fg={gray[500]}>{`Split ${allocationIndex + 1}`}</text>
                          <text
                            id={`transaction-remove-${allocation.postingId}`}
                            fg={gray[500]}
                            onMouseDown={() => removeAllocation(allocation.postingId)}
                          >
                            x
                          </text>
                        </box>
                        <box
                          id={`transaction-editor-${accountField}`}
                          style={{
                            width: "100%",
                            height: 1,
                            flexDirection: "row",
                            zIndex: accountOpen ? 101 : 0,
                          }}
                        >
                          <text fg={gray[500]} style={{ width: 2 }}>├─</text>
                          <box style={{ flexGrow: 1, minWidth: 0, height: 1 }}>
                            <FilterSelect
                              id={`transaction-allocation-${allocation.postingId}`}
                              options={options}
                              value={allocation.accountId ?? ""}
                              focused={transactionEditorFocus === accountField}
                              placeholder="Filter accounts"
                              onFocus={() => setTransactionEditorFocus(accountField)}
                              onOpenChange={(open) => setOpenTransactionAccountSelect(open ? allocation.postingId : null)}
                              onSelect={(accountId) => {
                                setTransactionDraft((draft) => draft ? {
                                  ...draft,
                                  allocations: draft.allocations.map((candidate) =>
                                    candidate.postingId === allocation.postingId
                                      ? { ...candidate, accountId }
                                      : candidate),
                                } : null)
                                setTransactionEditorError(null)
                              }}
                            />
                          </box>
                        </box>
                        <box
                          id={`transaction-editor-${amountField}`}
                          style={{ width: "100%", height: 1, flexDirection: "row" }}
                        >
                          <text fg={gray[500]} style={{ width: 2 }}>└─</text>
                          <box
                            style={{
                              flexGrow: 1,
                              minWidth: 0,
                              height: 1,
                              paddingRight: 1,
                              backgroundColor: transactionEditorFocus === amountField ? gray[700] : gray[800],
                            }}
                            onMouseDown={() => setTransactionEditorFocus(amountField)}
                          >
                            <input
                              focused={transactionEditorFocus === amountField}
                              value={allocation.amount}
                              placeholder="0.00"
                              textColor={gray[100]}
                              focusedTextColor={gray[100]}
                              backgroundColor={gray[800]}
                              focusedBackgroundColor={gray[700]}
                              cursorColor={gray[200]}
                              style={{ flexGrow: 1, minWidth: 0 }}
                              onMouseDown={() => setTransactionEditorFocus(amountField)}
                              onInput={(amount) => {
                                setTransactionDraft((draft) => draft ? {
                                  ...draft,
                                  allocations: draft.allocations.map((candidate) =>
                                    candidate.postingId === allocation.postingId
                                      ? { ...candidate, amount }
                                      : candidate),
                                } : null)
                                setTransactionEditorError(null)
                              }}
                            />
                          </box>
                        </box>
                      </box>
                    )
                  })}
                  <box
                    style={{
                      height: 2,
                      marginLeft: 1,
                      paddingTop: 1,
                      flexDirection: "row",
                      justifyContent: "space-between",
                    }}
                  >
                    <text fg={gray[500]}>Total remaining</text>
                    <text fg={remainingAmount === 0 ? gray[200] : gray[300]}>
                      {money(remainingAmount)}
                    </text>
                  </box>
                  <box style={{ height: 2, paddingTop: 1 }}>
                    <FormButton
                      id="transaction-editor-add"
                      label="Add split"
                      focused={transactionEditorFocus === "add"}
                      onPress={addAllocation}
                    />
                  </box>
                  {transactionEditorError ? (
                    <text fg={gray[300]} style={{ paddingLeft: 1 }}>{transactionEditorError}</text>
                  ) : null}
                  <box style={{ height: 2, paddingTop: 1 }}>
                    <FormButton
                      id="transaction-editor-save"
                      label="Save transaction"
                      focused={transactionEditorFocus === "save"}
                      disabled={!transactionCanSave}
                      onPress={saveTransaction}
                    />
                  </box>
                  <box style={{ height: 2, paddingTop: 1 }}>
                    <FormButton
                      id="transaction-editor-cancel"
                      label="Cancel"
                      focused={transactionEditorFocus === "cancel"}
                      onPress={cancelTransactionEditMode}
                    />
                  </box>
                </box>
              ) : transactionEditing ? (
                <box
                  key="transaction-simple-edit"
                  style={{ width: "100%", flexShrink: 0, flexDirection: "column" }}
                >
                  {editableNotesField}
                  <box style={{ height: 1 }} />
                  {showSplitSummary ? (
                    <box style={{ width: "100%", flexDirection: "column", paddingLeft: 1 }}>
                      <text fg={gray[500]}>Split counterparts</text>
                      {selectedCounterparts.map((posting) => (
                        <AccountAmountRow
                          key={posting.postingId}
                          name={state.accounts.find((account) => account.id === posting.accountId)?.name ?? "None"}
                          amount={money(posting.amount)}
                          amountWidth={splitCounterpartAmountWidth}
                        />
                      ))}
                    </box>
                  ) : (
                    <FormField
                      id="transaction-editor-counterpart"
                      label="Counterpart"
                      focused={normalEditorFocus === "counterpart"}
                      zIndex={openTransactionAccountSelect === "simple" ? 100 : 0}
                    >
                      <FilterSelect
                        id="transaction-counterpart"
                        options={[
                          { value: NO_ACCOUNT, label: "None", detail: "unassigned" },
                          ...creationParents
                            .filter((account) => account.id !== selectedViewPosting?.accountId)
                            .map((account) => ({
                              value: account.id,
                              label: account.name,
                              detail: account.type,
                              searchText: `${account.name} ${account.type}`,
                              group: account.type,
                              depth: account.depth,
                            })),
                        ]}
                        value={normalCounterpartAccountId}
                        focused={normalEditorFocus === "counterpart"}
                        placeholder="Filter accounts"
                        onFocus={() => setNormalEditorFocus("counterpart")}
                        onOpenChange={(open) => setOpenTransactionAccountSelect(open ? "simple" : null)}
                        onSelect={(accountId) => setNormalCounterpartAccountId(accountId)}
                      />
                    </FormField>
                  )}
                  <box
                    style={{
                      width: "100%",
                      flexShrink: 0,
                      paddingTop: 1,
                      flexDirection: "column",
                      gap: 1,
                    }}
                  >
                    <FormButton
                      id="transaction-editor-split"
                      label="Split transaction"
                      focused={normalEditorFocus === "split"}
                      onPress={beginTransactionEditing}
                    />
                    <FormButton
                      id="transaction-editor-save-mode"
                      label="Save transaction"
                      focused={normalEditorFocus === "save"}
                      onPress={saveTransactionEditMode}
                    />
                    <FormButton
                      id="transaction-editor-cancel-mode"
                      label="Cancel"
                      focused={normalEditorFocus === "cancel"}
                      onPress={cancelTransactionEditMode}
                    />
                  </box>
                </box>
              ) : (
                <box
                  key={`transaction-read-only-${selectedTransaction?.source_id ?? "none"}`}
                  style={{ width: "100%", flexShrink: 0, flexDirection: "column" }}
                >
                  <box style={{ width: "100%", flexDirection: "column" }}>
                    <text fg={gray[500]}>Notes</text>
                    <text fg={gray[100]} wrapMode="word" style={{ width: "100%" }}>
                      {selectedTransaction?.notes || "None"}
                    </text>
                  </box>
                  {selectedTransaction?.logs?.length ? (
                    <>
                      <box style={{ height: 1 }} />
                      <box style={{ width: "100%", flexDirection: "column" }}>
                        <text fg={gray[500]}>Activity</text>
                        {(selectedTransaction.logs ?? []).map((log, index) => (
                          <text key={`${log.created_at}-${index}`} fg={gray[400]} wrapMode="word">
                            - {log.message}
                          </text>
                        ))}
                      </box>
                    </>
                  ) : null}
                  <box style={{ height: 1 }} />
                  {selectedCounterparts.length > 1 ? (
                    <box style={{ width: "100%", flexDirection: "column" }}>
                      <text fg={gray[500]}>Split counterparts</text>
                      {selectedCounterparts.map((posting) => (
                        <AccountAmountRow
                          key={posting.postingId}
                          name={state.accounts.find((account) => account.id === posting.accountId)?.name ?? "None"}
                          amount={money(posting.amount)}
                          amountWidth={splitCounterpartAmountWidth}
                          bright
                        />
                      ))}
                    </box>
                  ) : (
                    <box style={{ width: "100%", flexDirection: "column" }}>
                      <text fg={gray[500]}>Counterpart</text>
                      <text fg={gray[100]} wrapMode="none">
                        {state.accounts.find((account) => account.id === selectedCounterparts[0]?.accountId)?.name ?? "None"}
                      </text>
                    </box>
                  )}
                  <box style={{ width: "100%", height: 2, paddingTop: 1 }}>
                    <box
                      id="transaction-editor-edit"
                      style={{
                        width: "100%",
                        height: 1,
                        flexDirection: "row",
                        justifyContent: "center",
                        backgroundColor: normalEditorFocus === "edit" ? selectionBackground : gray[800],
                      }}
                      onMouseDown={beginTransactionEditMode}
                    >
                      <text fg={normalEditorFocus === "edit" ? gray[100] : gray[300]}>Edit transaction</text>
                    </box>
                  </box>
                </box>
              )}
            </scrollbox>

          </box>
        ) : null}

        <box style={{ flexGrow: 1, flexDirection: "row" }}>
        {showAccountSidebar ? <scrollbox
          ref={accountListRef}
          scrollX={false}
          scrollY
          title=" Accounts "
          style={{
            width: ACCOUNT_SIDEBAR_WIDTH,
            height: "100%",
            border: ["right"],
            borderColor: gray[700],
            focusedBorderColor: gray[700],
            flexDirection: "column",
            paddingTop: 1,
            paddingLeft: 1,
            paddingRight: 1,
            verticalScrollbarOptions: { visible: false },
          }}
        >
          {financialAccounts.length === 0 ? <text fg={gray[600]}>No accounts</text> : null}
          {financialAccounts.map((account, index) => {
            const selected = account.id === state.selectedAccountId
            const virtual = isVirtualAccountId(account.id)
            const emptyTopLevelGroup = account.depth === 0 && !account.has_children && !virtual
            return (
              <box
                key={account.id}
                id={`account-${account.external_id ?? account.id}`}
                style={{
                  height: 1,
                  marginTop: accountRowMarginTop(financialAccounts, index),
                  paddingLeft: 1,
                  paddingRight: 1,
                  flexDirection: "row",
                  alignItems: "center",
                  backgroundColor: selected ? selectionBackground : gray[900],
                }}
                onMouseDown={transactionEditing || transactionDraft ? undefined : () => {
                  accountListRef.current?.focus()
                  selectAccount(registry, account.id)
                }}
              >
                <text
                  fg={
                    selected
                      ? gray[100]
                      : account.has_children || emptyTopLevelGroup ? gray[500] : gray[300]
                  }
                  wrapMode="none"
                >
                  {short(
                    `${"  ".repeat(account.depth)}${account.has_children
                      ? collapsedAccountIds.has(account.id) ? "▸ " : "▾ "
                      : emptyTopLevelGroup ? "▾ "
                      : account.depth > 0 ? "  " : ""}${account.name}`,
                    22,
                  )}
                </text>
              </box>
            )
          })}
        </scrollbox> : null}

        <box style={{ flexGrow: 1, flexDirection: "column", padding: 1, gap: 1 }}>
          <box
            style={{
              height: showRegisterBalance ? 2 : 1,
              flexDirection: "column",
            }}
          >
            <box style={{ height: 1, flexDirection: "row", justifyContent: "space-between" }}>
              <text fg={gray[100]}>
              <strong>
                {deletingAccount
                  ? "Delete account"
                  : state.creatingAccount
                  ? editingAccount ? "Edit account" : "Create account"
                  : state.assigning
                    ? state.choosingPosting
                      ? "Choose posting"
                      : state.editingPostingId
                        ? "Change account"
                        : "Assign account"
                    : (selectedAccount?.name ?? "Transactions")}
              </strong>
              {deletingAccount
                ? `  /  ${deletingAccount.name}`
                : state.creatingAccount
                ? newAccountParent
                  ? `  /  ${newAccountParent.name}  /  ${newAccountParent.type}`
                  : `  /  new root  /  ${state.newAccountType}`
                : state.assigning
                ? selectedTransaction
                  ? `  /  ${short(selectedTransaction.payee, 24)}  /  ${money(Math.abs(assignmentAmount))}`
                  : ""
                : selectedAccount && !virtualAccountSelected
                  ? `  /  ${selectedAccount.type}`
                  : ""}
              </text>
              {registerVisible ? (
                <input
                  id="transaction-search"
                  focused={searchFocused && !transactionEditing && !transactionDraft}
                  value={searchDraft}
                  placeholder="Search transactions"
                  textColor={gray[100]}
                  focusedTextColor={gray[100]}
                  backgroundColor={gray[800]}
                  focusedBackgroundColor={gray[700]}
                  cursorColor={gray[200]}
                  style={{ width: 30, paddingLeft: 1, paddingRight: 1 }}
                  onMouseDown={transactionEditing || transactionDraft
                    ? undefined
                    : () => setSearchFocused(true)}
                  onInput={setSearchDraft}
                />
              ) : (
                <text fg={gray[500]}>
                  {deletingAccount
                    ? deletionContainsSyncedAccount
                      ? "esc cancel"
                      : "enter confirm / esc cancel"
                    : state.creatingAccount
                    ? `tab next / enter ${editingAccount ? "save" : "create"} / esc cancel`
                    : state.assigning
                      ? state.choosingPosting
                        ? "enter choose / esc cancel"
                        : "a new / enter assign / esc cancel"
                      : ""}
                </text>
              )}
            </box>
            {showRegisterBalance ? (
              <text fg={selectedAccount && accountBalance(selectedAccount) < 0 ? gray[300] : gray[200]}>
                {money(accountBalance(selectedAccount))}
              </text>
            ) : null}
          </box>

          {deletingAccount ? (
            <box
              key="account-deletion"
              style={{
                width: "80%",
                maxWidth: 60,
                alignSelf: "center",
                flexGrow: 1,
                paddingTop: 2,
                flexDirection: "column",
                gap: 1,
              }}
            >
              <text fg={gray[100]}>
                <strong>Delete {deletingAccount.name}?</strong>
              </text>
              <text fg={gray[500]}>
                {deletionAccounts.length === 1
                  ? "This account will be permanently deleted."
                  : `This account and ${deletionAccounts.length - 1} descendants will be permanently deleted.`}
              </text>
              {deletionContainsSyncedAccount ? (
                <text fg={gray[300]}>Synced accounts cannot be deleted locally.</text>
              ) : (
                <>
                  {([
                    ["Keep transactions", "Affected postings become None"],
                    ["Delete transactions", "Affected transaction history is permanently erased"],
                  ] as const).map(([label, description], index) => {
                    const selected = deletionChoice === index
                    const disabled = deletingCategory && index === 1
                    return (
                      <box
                        key={label}
                        id={`delete-account-choice-${index}`}
                        style={{
                          height: 2,
                          paddingLeft: 1,
                          paddingRight: 1,
                          flexDirection: "column",
                          backgroundColor: selected && !disabled ? selectionBackground : gray[900],
                        }}
                        onMouseDown={disabled ? undefined : () => setDeletionChoice(index as 0 | 1)}
                      >
                        <text fg={disabled ? gray[600] : selected ? gray[100] : gray[300]}>{label}</text>
                        <text fg={gray[600]}>
                          {disabled ? "Categories must preserve their transaction history" : description}
                        </text>
                      </box>
                    )
                  })}
                </>
              )}
            </box>
          ) : state.creatingAccount ? (
            <box
              key={editingAccount ? "account-editing" : "account-creation"}
              style={{
                width: "85%",
                maxWidth: 64,
                alignSelf: "center",
                flexGrow: 1,
                paddingTop: 1,
                flexDirection: "column",
                gap: 1,
              }}
            >
              <FormField label="Name" focused={creationField === "name" && openCreationSelect === null}>
                <input
                  id={editingAccount ? "edit-account-name" : "new-account-name"}
                  focused={creationField === "name" && openCreationSelect === null}
                  value={state.newAccountName}
                  placeholder="e.g. Vacation"
                  textColor={gray[100]}
                  focusedTextColor={gray[100]}
                  backgroundColor={gray[800]}
                  focusedBackgroundColor={gray[700]}
                  cursorColor={gray[200]}
                  style={{ width: "100%", paddingLeft: 1, paddingRight: 1 }}
                  onMouseDown={() => setCreationField("name")}
                  onInput={(value) => setNewAccountName(registry, value)}
                  onSubmit={() => createAccount(registry)}
                />
              </FormField>

              <FormField
                label={editingSyncedAccount ? "Parent (managed by sync)" : "Parent"}
                focused={creationField === "parent"}
                zIndex={openCreationSelect === "parent" ? 100 : 0}
              >
                <FilterSelect
                  id="new-account-parent"
                  options={parentOptions}
                  value={state.newAccountParentId ?? NO_PARENT}
                  focused={creationField === "parent"}
                  disabled={editingSyncedAccount}
                  placeholder="Filter accounts"
                  onFocus={() => setCreationField("parent")}
                  onOpenChange={(open) =>
                    setOpenCreationSelect((current) => open ? "parent" : current === "parent" ? null : current)}
                  onSelect={(value) => setNewAccountParent(registry, value === NO_PARENT ? null : value)}
                />
              </FormField>

              <FormField
                label={editingSyncedAccount ? "Type (managed by sync)" : "Type"}
                focused={creationField === "type"}
                zIndex={openCreationSelect === "type" ? 100 : 0}
              >
                <FilterSelect
                  id="new-account-type"
                  options={typeOptions}
                  value={state.newAccountType}
                  focused={creationField === "type"}
                  disabled={editingSyncedAccount || state.newAccountParentId !== null}
                  placeholder="Filter account types"
                  onFocus={() => setCreationField("type")}
                  onOpenChange={(open) =>
                    setOpenCreationSelect((current) => open ? "type" : current === "type" ? null : current)}
                  onSelect={(value) => setNewAccountType(registry, value)}
                />
              </FormField>

              <FormButton
                id={editingAccount ? "save-account" : "create-account"}
                label={editingAccount ? "Save account" : "Create account"}
                focused={creationField === "button"}
                onPress={() => createAccount(registry)}
              />
            </box>
          ) : state.assigning ? (
            <box key="assignment" style={{ flexGrow: 1, flexDirection: "column" }}>
              {state.choosingPosting ? (
                <>
                  <text fg={gray[500]}>Which side do you want to change?</text>
                  {selectedTransaction?.other_postings.map((posting, index) => (
                    <box
                      key={posting.posting_id}
                      id={`choose-posting-${posting.posting_id}`}
                      style={{
                        height: 1,
                        paddingLeft: 2,
                        paddingRight: 2,
                        flexDirection: "row",
                        justifyContent: "space-between",
                        backgroundColor: index === state.selectedPostingIndex ? selectionBackground : gray[900],
                      }}
                      onMouseDown={() => choosePosting(registry, posting.posting_id)}
                    >
                      <text fg={index === state.selectedPostingIndex ? gray[100] : gray[300]}>
                        {posting.account_name}
                      </text>
                      <text fg={gray[500]}>{money(posting.amount)}</text>
                    </box>
                  ))}
                </>
              ) : (
                <>
                  <text fg={gray[500]}>Where did this money go or come from?</text>
                  {assignmentTree.map((account) => (
                    <box
                      key={account.id}
                      id={`assign-${account.external_id ?? account.id}`}
                      style={{
                        height: 1,
                        paddingLeft: 2,
                        paddingRight: 2,
                        flexDirection: "row",
                        alignItems: "center",
                        justifyContent: "space-between",
                        backgroundColor: account.id === selectedAssignment?.id ? selectionBackground : gray[900],
                      }}
                      onMouseDown={() => assignToAccount(registry, account.id)}
                    >
                      <text fg={account.id === selectedAssignment?.id ? gray[100] : account.has_children ? gray[500] : gray[300]}>
                        {`${"  ".repeat(account.depth)}${account.has_children ? "▾ " : "  "}${account.name}`}
                      </text>
                      <text fg={gray[600]}>{account.type}</text>
                    </box>
                  ))}
                </>
              )}
            </box>
          ) : (
            <box key="transactions" style={{ flexGrow: 1, flexDirection: "column" }}>
              <box style={{ width: "100%", height: 1, flexDirection: "row", gap: 2 }}>
                <text fg={gray[500]} style={{ width: 11, paddingLeft: 1 }}><em>Date</em></text>
                <text
                  fg={gray[500]}
                  style={flexibleTableColumn}
                >
                  <em>Payee</em>
                </text>
                {showTransactionAccountColumn ? (
                  <text fg={gray[500]} style={flexibleTableColumn}><em>Account</em></text>
                ) : null}
                {showTransactionCounterpartColumn ? (
                  <text fg={gray[500]} style={{ width: showTransactionAccountColumn ? 13 : 15 }}><em>Counterpart</em></text>
                ) : null}
                <box style={{ width: 12, height: 1, flexDirection: "row", justifyContent: "flex-end" }}>
                  <text fg={gray[500]}><em>Amount</em></text>
                </box>
              </box>

              <scrollbox
                ref={transactionListRef}
                scrollX={false}
                scrollY
                style={{
                  flexGrow: 1,
                  flexDirection: "column",
                  verticalScrollbarOptions: { visible: false },
                }}
              >
                {!state.busy && state.transactions.length === 0 ? (
                  <text fg={gray[600]} style={{ padding: 1 }}>No transactions for this account.</text>
                ) : null}
                {state.transactions.map((transaction, index) => {
                  const startsDay = index === 0 || state.transactions[index - 1]?.date !== transaction.date
                  const other = transaction.other_postings
                    .map((posting) => posting.account_name ?? "None")
                    .join(", ")
                  const otherLabel = transaction.other_postings.length > 1
                    ? "(split)"
                    : other
                    ? transaction.unallocated_amount === 0
                      ? other
                      : `${other} + open`
                    : "None"
                  const payeeLabel = `${transaction.payee || "(no payee)"}${transaction.pending ? " *" : ""}`
                  const accountName = transaction.account_id === null
                    ? "None"
                    : state.accounts.find((account) => account.id === transaction.account_id)?.name ?? "Unknown"
                  const selected = state.focus === "transactions" && index === state.selectedTransactionIndex
                  return (
                    <box
                      key={`${transaction.source_id}-${transaction.posting_id}`}
                      id={`transaction-${transaction.source_id}-${transaction.posting_id}`}
                      style={{
                        width: "100%",
                        height: 1,
                        flexDirection: "row",
                        gap: 2,
                        alignItems: "center",
                        backgroundColor: selected ? selectionBackground : gray[900],
                      }}
                      onMouseDown={transactionEditing || transactionDraft ? undefined : () => {
                        transactionListRef.current?.focus()
                        if (transactionEditorOpen) {
                          setNormalEditorFocus("register")
                        }
                        selectTransaction(registry, index)
                      }}
                    >
                      <text fg={startsDay ? gray[200] : gray[600]} style={{ width: 11, paddingLeft: 1 }}>
                        {transaction.date}
                      </text>
                      <box
                        ref={selected ? payeeAnchorRef : undefined}
                        style={{ ...flexibleTableColumn, height: 1 }}
                        onMouseDown={selected ? openPayeePicker : undefined}
                      >
                        <text fg={gray[200]} wrapMode="none">{payeeLabel}</text>
                      </box>
                      {showTransactionAccountColumn ? (
                        <text
                          fg={transaction.account_id === null ? gray[600] : gray[200]}
                          wrapMode="none"
                          style={flexibleTableColumn}
                        >
                          {accountName}
                        </text>
                      ) : null}
                      {showTransactionCounterpartColumn ? (
                        <box
                          ref={selected ? counterpartAnchorRef : undefined}
                          style={{ width: showTransactionAccountColumn ? 13 : 15, height: 1 }}
                          onMouseDown={selected ? openCounterpartPicker : undefined}
                        >
                          <text fg={other ? gray[200] : gray[600]}>
                            {short(otherLabel, showTransactionAccountColumn ? 12 : 14)}
                          </text>
                        </box>
                      ) : null}
                      <box style={{ width: 12, height: 1, flexDirection: "row", justifyContent: "flex-end" }}>
                        <text fg={transaction.amount < 0 ? gray[300] : gray[200]}>
                          {money(transaction.amount).replace("$", "")}
                        </text>
                      </box>
                    </box>
                  )
                })}
              </scrollbox>
              <Popover
                open={counterpartPickerOpen}
                anchorRef={counterpartAnchorRef}
                height={counterpartVisibleCount + 1}
                width={30}
                align="right"
                anchorMode="overlay"
              >
                <box
                  style={{
                    width: "100%",
                    height: counterpartVisibleCount + 1,
                    flexDirection: "column",
                    backgroundColor: gray[800],
                  }}
                >
                  <input
                    id="inline-counterpart-filter"
                    focused
                    value={counterpartFilter}
                    placeholder="Filter counterparts"
                    textColor={gray[100]}
                    focusedTextColor={gray[100]}
                    backgroundColor={gray[700]}
                    focusedBackgroundColor={gray[700]}
                    cursorColor={gray[200]}
                    style={{ width: "100%" }}
                    onInput={(value) => {
                      setCounterpartFilter(value)
                      setCounterpartHighlight(0)
                    }}
                  />
                  {visibleCounterpartOptions.length === 0 ? (
                    <text fg={gray[600]}>No matches</text>
                  ) : visibleCounterpartOptions.map((account, visibleIndex) => {
                    const index = counterpartVisibleStart + visibleIndex
                    const highlighted = index === counterpartHighlight
                    return (
                      <box
                        key={account.id}
                        id={`inline-counterpart-${account.id}`}
                        style={{
                          height: 1,
                          flexDirection: "row",
                          justifyContent: "space-between",
                          backgroundColor: highlighted ? gray[700] : gray[800],
                        }}
                        onMouseDown={() => selectInlineCounterpart(account.id)}
                      >
                        <text fg={highlighted ? gray[100] : gray[300]}>
                          {`${"  ".repeat(account.depth)}${account.name}`}
                        </text>
                        <text fg={gray[600]}>{account.type}</text>
                      </box>
                    )
                  })}
                </box>
              </Popover>
              <Popover
                open={payeePickerOpen}
                anchorRef={payeeAnchorRef}
                height={payeeVisibleCount + 1}
                width={30}
                anchorMode="overlay"
              >
                <box
                  style={{
                    width: "100%",
                    height: payeeVisibleCount + 1,
                    flexDirection: "column",
                    backgroundColor: gray[800],
                  }}
                >
                  <input
                    id="inline-payee-filter"
                    focused
                    value={payeeFilter}
                    placeholder="Set payee"
                    textColor={gray[100]}
                    focusedTextColor={gray[100]}
                    backgroundColor={gray[700]}
                    focusedBackgroundColor={gray[700]}
                    cursorColor={gray[200]}
                    style={{ width: "100%" }}
                    onInput={(value) => {
                      setPayeeFilter(value)
                      setPayeeHighlight(0)
                    }}
                  />
                  {visiblePayeeOptions.length === 0 ? (
                    <text fg={gray[600]}>
                      {normalizedPayeeFilter ? "No prefix matches" : "Type to search"}
                    </text>
                  ) : visiblePayeeOptions.map((payee, visibleIndex) => {
                    const index = payeeVisibleStart + visibleIndex
                    const highlighted = index === payeeHighlight
                    return (
                      <box
                        key={payee}
                        id={`inline-payee-${payee}`}
                        style={{
                          height: 1,
                          backgroundColor: highlighted ? gray[700] : gray[800],
                        }}
                        onMouseDown={() => selectInlinePayee(payee)}
                      >
                        <text fg={highlighted ? gray[100] : gray[300]}>{payee}</text>
                      </box>
                    )
                  })}
                </box>
              </Popover>
            </box>
          )}
        </box>
        </box>
        </>
        )}
      </box>

      {syncOptionsOpen ? (
        <box
          style={{
            position: "absolute",
            top: 0,
            left: 0,
            width: "100%",
            height: "100%",
            zIndex: 2100,
            alignItems: "center",
            justifyContent: "center",
          }}
        >
          <box
            title=" Sync options "
            style={{
              width: 42,
              border: true,
              borderColor: gray[600],
              backgroundColor: gray[900],
              flexDirection: "column",
              padding: 1,
              gap: 1,
            }}
          >
            <text fg={gray[500]}>Imports from January 1 through this date.</text>
            <FormField label="Through date" focused>
              <input
                id="sync-through-date"
                focused
                value={syncEndDate}
                placeholder="YYYY-MM-DD"
                textColor={gray[100]}
                focusedTextColor={gray[100]}
                backgroundColor={gray[800]}
                focusedBackgroundColor={gray[700]}
                cursorColor={gray[200]}
                style={{ width: "100%", paddingLeft: 1, paddingRight: 1 }}
                onInput={(value) => {
                  setSyncEndDate(value)
                  setSyncDateError(null)
                }}
                onSubmit={runConfiguredSync}
              />
            </FormField>
            {syncDateError ? <text fg={gray[300]} style={{ marginLeft: 1 }}>{syncDateError}</text> : null}
            <FormButton
              id="sync-create-categories"
              label="Add income & expense accounts"
              focused={false}
              disabled={state.busy}
              onPress={() => createDebugCategories(registry)}
            />
            <box style={{ width: "100%", flexDirection: "row", gap: 1 }}>
              <box style={{ width: "50%" }}>
                <FormButton label="Cancel" focused={false} onPress={() => setSyncOptionsOpen(false)} />
              </box>
              <box style={{ width: "50%" }}>
                <FormButton label="Sync" focused={false} disabled={state.busy} onPress={runConfiguredSync} />
              </box>
            </box>
            <text fg={gray[600]}>enter sync   esc cancel</text>
          </box>
        </box>
      ) : null}

      {syncLogOpen ? (
        <box
          style={{
            position: "absolute",
            top: 0,
            left: 0,
            width: "100%",
            height: "100%",
            zIndex: 2000,
            alignItems: "center",
            justifyContent: "center",
          }}
        >
          <box
            title=" Transaction sync "
            style={{
              width: "80%",
              maxWidth: 90,
              height: "75%",
              border: true,
              borderColor: gray[600],
              backgroundColor: gray[900],
              flexDirection: "column",
              paddingLeft: 1,
              paddingRight: 1,
            }}
          >
            <text fg={syncLog.running ? gray[200] : gray[400]}>
              <strong>{syncLog.running ? "Syncing" : "Sync complete"}</strong>
            </text>
            <box style={{ height: 1 }} />
            <scrollbox
              ref={syncLogRef}
              scrollX={false}
              scrollY
              style={{
                flexGrow: 1,
                verticalScrollbarOptions: { visible: true },
              }}
            >
              {syncLog.lines.map((line, index) => (
                <text key={index} fg={line.startsWith("Sync failed") ? gray[300] : gray[400]} wrapMode="word">
                  {line}
                </text>
              ))}
            </scrollbox>
            <box style={{ height: 1, flexDirection: "row", justifyContent: "space-between" }}>
              <text fg={gray[600]}>
                {syncLog.running ? "sync continues in background" : "sync complete"}
              </text>
              <text
                id="sync-log-close"
                fg={gray[300]}
                onMouseDown={() => setSyncLogOpen(false)}
              >
                [close] esc/enter
              </text>
            </box>
          </box>
        </box>
      ) : null}

      <box
        style={{
          height: 1,
          flexDirection: "row",
          justifyContent: "space-between",
          alignItems: "center",
          backgroundColor: gray[800],
        }}
      >
        <text fg={gray[600]}>
          {screen === "plugins"
            ? "plugin settings"
            : screen === "debug-sync"
              ? "tab field   space toggle   enter activate"
            : transactionEditing
              ? "transaction editor   tab field   esc read-only"
            : transactionEditorOpen
              ? "tab field   ↑/↓ move transactions when register is focused   esc close"
              : `${showAccountSidebar ? "tab pane   " : ""}↑/↓ move   a new   e edit   ${showAccountSidebar ? "d delete   " : ""}pgup/pgdn page`}
        </text>
        <text fg={gray[600]}>{screen !== "ledger" || transactionEditorOpen ? "" : "p payee   c counterpart   r refresh   s sync   q quit"}</text>
      </box>
    </box>
    </PopoverProvider>
  )
}
