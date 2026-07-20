import { RegistryContext, useAtomValue } from "@effect-atom/atom-react"
import { useKeyboard, useRenderer } from "@opentui/react"
import { useContext, useEffect } from "react"

import {
  type Account,
  accountTreeAtom,
  assignmentAccountsAtom,
  assignToAccount,
  beginAccountCreation,
  beginAssignment,
  cancelAccountCreation,
  cancelAssignment,
  choosePosting,
  createAccount,
  creationParentsAtom,
  financialAccountsAtom,
  initialize,
  moveSelection,
  nextPage,
  previousPage,
  recordPerformance,
  refresh,
  selectAccount,
  selectTransaction,
  selectedAccountAtom,
  selectedTransactionAtom,
  setNewAccountName,
  setNewAccountParent,
  setNewAccountType,
  stateAtom,
  sync,
  toggleFocus,
} from "./store"

const money = (amount: number) => {
  const absolute = Math.abs(amount)
  const formatted = `$${Math.floor(absolute / 100).toLocaleString()}.${String(absolute % 100).padStart(2, "0")}`
  return amount < 0 ? `-${formatted}` : formatted
}

const accountBalance = (account: Account) =>
  account.type === "income" || account.type === "liability" || account.type === "equity"
    ? -account.balance
    : account.balance

const short = (value: string | null, width: number) => {
  const text = value || "(no payee)"
  return text.length > width ? `${text.slice(0, width - 1)}…` : text
}

export function App() {
  const renderer = useRenderer()
  const registry = useContext(RegistryContext)
  const state = useAtomValue(stateAtom)
  const selectedAccount = useAtomValue(selectedAccountAtom)
  const financialAccounts = useAtomValue(financialAccountsAtom)
  const selectedTransaction = useAtomValue(selectedTransactionAtom)
  const assignmentAccounts = useAtomValue(assignmentAccountsAtom)
  const creationParents = useAtomValue(creationParentsAtom)
  const accountTree = useAtomValue(accountTreeAtom)
  const assignmentTree = accountTree.filter((account) => account.id !== state.selectedAccountId)
  const selectedAssignment = assignmentAccounts[state.selectedAssignmentIndex] ?? null
  const editingPosting = selectedTransaction?.other_postings.find(
    (posting) => posting.posting_id === state.editingPostingId,
  )
  const assignmentAmount = editingPosting?.amount ?? selectedTransaction?.unallocated_amount ?? 0

  useEffect(() => {
    initialize(registry)
  }, [registry])

  useEffect(() => {
    if (!state.trace) return
    recordPerformance("frontend.render_commit", {
      trace_id: state.trace.id,
      source_event: state.trace.event,
      rows: state.trace.rows,
      elapsed_ms: performance.now() - state.trace.startedAt,
    })
  }, [state.trace])

  useKeyboard((key) => {
    if (state.creatingAccount) {
      if (key.name === "escape") cancelAccountCreation(registry)
      return
    }
    if (key.name === "q") renderer.destroy()
    if (key.name === "escape") {
      if (state.assigning) cancelAssignment(registry)
      else renderer.destroy()
    }
    if (key.name === "up" || key.name === "k") moveSelection(registry, -1)
    if (key.name === "down" || key.name === "j") moveSelection(registry, 1)
    if (key.name === "tab") toggleFocus(registry)
    if (key.name === "c") beginAssignment(registry)
    if (key.name === "a" && state.assigning) beginAccountCreation(registry)
    if (key.name === "pageDown" || key.name === "pagedown" || key.name === "n") nextPage(registry)
    if (key.name === "pageUp" || key.name === "pageup" || key.name === "p") previousPage(registry)
    if (key.name === "return" || key.name === "enter") {
      if (state.assigning) {
        if (state.choosingPosting) choosePosting(registry)
        else assignToAccount(registry)
      }
      else if (state.focus === "transactions") beginAssignment(registry)
    }
    if (key.name === "s") sync(registry)
    if (key.name === "r") refresh(registry)
  })

  return (
    <box
      style={{
        width: "100%",
        height: "100%",
        flexDirection: "column",
        backgroundColor: "#0e1113",
      }}
    >
      <box
        style={{
          height: 3,
          paddingLeft: 2,
          paddingRight: 2,
          flexDirection: "row",
          alignItems: "center",
          justifyContent: "space-between",
          backgroundColor: "#17201c",
        }}
      >
        <text fg="#8de0b5">
          <strong>MONET</strong>  ledger probe
        </text>
        <box style={{ flexDirection: "row", gap: 2 }}>
          <text fg={state.error ? "#ee8f86" : state.busy ? "#e7c66b" : "#82948b"}>
            {state.error ?? state.message}
          </text>
          <text id="sync-all" fg="#8de0b5" onMouseDown={() => sync(registry)}>
            [s] sync
          </text>
        </box>
      </box>

      <box style={{ flexGrow: 1, flexDirection: "row" }}>
        <box
          title=" Accounts "
          style={{
            width: 22,
            border: ["right"],
            borderColor: "#29332f",
            flexDirection: "column",
            paddingTop: 1,
            paddingLeft: 1,
            paddingRight: 1,
          }}
        >
          {financialAccounts.length === 0 ? <text fg="#66736d">No accounts</text> : null}
          {financialAccounts.map((account) => {
            const selected = account.id === state.selectedAccountId
            return (
              <box
                key={account.id}
                id={`account-${account.external_id ?? account.id}`}
                style={{
                  height: 2,
                  paddingLeft: 1,
                  paddingRight: 1,
                  flexDirection: "row",
                  alignItems: "center",
                  justifyContent: "space-between",
                  backgroundColor: selected ? "#29463a" : "#0e1113",
                }}
                onMouseDown={() => selectAccount(registry, account.id)}
              >
                <text fg={selected ? "#d9f7e7" : account.has_children ? "#718078" : "#aebbb5"}>
                  {short(`${"  ".repeat(account.depth)}${account.has_children ? "▾ " : ""}${account.name}`, 13)}
                </text>
                <text fg="#66736d">{account.type.slice(0, 3)}</text>
              </box>
            )
          })}
        </box>

        <box style={{ flexGrow: 1, flexDirection: "column", padding: 1, gap: 1 }}>
          <box style={{ height: 2, flexDirection: "row", justifyContent: "space-between" }}>
            <text fg="#ecf3ef">
              <strong>
                {state.creatingAccount
                  ? "Create category"
                  : state.assigning
                    ? state.choosingPosting
                      ? "Choose posting"
                      : state.editingPostingId
                        ? "Change account"
                        : "Assign account"
                    : (selectedAccount?.name ?? "Transactions")}
              </strong>
              {state.creatingAccount
                ? "  /  income or expense account"
                : state.assigning
                ? selectedTransaction
                  ? `  /  ${short(selectedTransaction.payee, 24)}  /  ${money(Math.abs(assignmentAmount))}`
                  : ""
                : selectedAccount
                  ? `  /  ${selectedAccount.type}`
                  : ""}
            </text>
            <text fg={selectedAccount && accountBalance(selectedAccount) < 0 ? "#ee9b90" : "#91ddb5"}>
              {state.creatingAccount
                ? "enter create / esc back"
                : state.assigning
                  ? state.choosingPosting
                    ? "enter choose / esc cancel"
                    : "a new / enter assign / esc cancel"
                : selectedAccount
                  ? `${money(accountBalance(selectedAccount))} balance`
                  : ""}
            </text>
          </box>

          {state.creatingAccount ? (
            <box style={{ flexGrow: 1, flexDirection: "column" }}>
              <text fg="#82948b">Name</text>
              <input
                id="new-account-name"
                focused
                value={state.newAccountName}
                placeholder="e.g. Restaurants"
                textColor="#ecf3ef"
                focusedTextColor="#ecf3ef"
                backgroundColor="#18201d"
                focusedBackgroundColor="#1d2924"
                cursorColor="#8de0b5"
                onInput={(value) => setNewAccountName(registry, value)}
                onSubmit={() => createAccount(registry)}
              />

              <text fg="#82948b">Type</text>
              <box style={{ flexDirection: "row", gap: 1 }}>
                {(["expense", "income"] as const).map((accountType) => (
                  <box
                    key={accountType}
                    id={`new-account-type-${accountType}`}
                    style={{
                      width: 14,
                      paddingLeft: 1,
                      backgroundColor: state.newAccountType === accountType ? "#29463a" : "#151a18",
                    }}
                    onMouseDown={() => setNewAccountType(registry, accountType)}
                  >
                    <text fg={state.newAccountType === accountType ? "#d9f7e7" : "#82948b"}>
                      {accountType}
                    </text>
                  </box>
                ))}
              </box>

              <text fg="#82948b">Parent (optional)</text>
              <box
                id="new-account-parent-none"
                style={{
                  height: 1,
                  paddingLeft: 2,
                  backgroundColor: state.newAccountParentId === null ? "#29463a" : "#111614",
                }}
                onMouseDown={() => setNewAccountParent(registry, null)}
              >
                <text fg={state.newAccountParentId === null ? "#d9f7e7" : "#82948b"}>No parent</text>
              </box>
              {creationParents.map((account) => {
                return (
                  <box
                    key={account.id}
                    id={`new-account-parent-${account.id}`}
                    style={{
                      height: 1,
                      paddingLeft: 2,
                      flexDirection: "row",
                      justifyContent: "space-between",
                      backgroundColor: state.newAccountParentId === account.id ? "#29463a" : "#111614",
                    }}
                    onMouseDown={() => setNewAccountParent(registry, account.id)}
                  >
                    <text
                      fg={
                        state.newAccountParentId === account.id
                          ? "#d9f7e7"
                          : "#b9c5bf"
                      }
                    >
                      {`${"  ".repeat(account.depth)}${account.has_children ? "▾ " : ""}${account.name}`}
                    </text>
                  </box>
                )
              })}

              <box
                id="create-account"
                style={{ width: 18, height: 2, paddingLeft: 2, backgroundColor: "#29463a" }}
                onMouseDown={() => createAccount(registry)}
              >
                <text fg="#d9f7e7">Create account</text>
              </box>
            </box>
          ) : state.assigning ? (
            <box style={{ flexGrow: 1, flexDirection: "column" }}>
              {state.choosingPosting ? (
                <>
                  <text fg="#82948b">Which side do you want to change?</text>
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
                        backgroundColor: index === state.selectedPostingIndex ? "#29463a" : "#111614",
                      }}
                      onMouseDown={() => choosePosting(registry, posting.posting_id)}
                    >
                      <text fg={index === state.selectedPostingIndex ? "#d9f7e7" : "#b9c5bf"}>
                        {posting.account_name}
                      </text>
                      <text fg="#82948b">{money(posting.amount)}</text>
                    </box>
                  ))}
                </>
              ) : (
                <>
                  <text fg="#82948b">Where did this money go or come from?</text>
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
                        backgroundColor: account.id === selectedAssignment?.id ? "#29463a" : "#111614",
                      }}
                      onMouseDown={() => assignToAccount(registry, account.id)}
                    >
                      <text fg={account.id === selectedAssignment?.id ? "#d9f7e7" : account.has_children ? "#718078" : "#b9c5bf"}>
                        {`${"  ".repeat(account.depth)}${account.has_children ? "▾ " : ""}${account.name}`}
                      </text>
                      <text fg="#66736d">{account.type}</text>
                    </box>
                  ))}
                </>
              )}
            </box>
          ) : (
            <>
              <box style={{ height: 1, flexDirection: "row", backgroundColor: "#1a211e" }}>
                <text fg="#82948b" style={{ width: 11, paddingLeft: 1 }}>Date</text>
                <text fg="#82948b" style={{ flexGrow: 1 }}>Payee</text>
                <text fg="#82948b" style={{ width: 15 }}>Other account</text>
                <text fg="#82948b" style={{ width: 12 }}>Amount</text>
                <text fg="#82948b" style={{ width: 8 }}>State</text>
              </box>

              <box style={{ flexGrow: 1, flexDirection: "column" }}>
                {!state.busy && state.transactions.length === 0 ? (
                  <text fg="#66736d" style={{ padding: 1 }}>No transactions for this account.</text>
                ) : null}
                {state.transactions.map((transaction, index) => {
                  const other = transaction.other_postings.map((posting) => posting.account_name).join(", ")
                  const otherLabel = other
                    ? transaction.unallocated_amount === 0
                      ? other
                      : `${other} + open`
                    : "Needs account"
                  const selected = state.focus === "transactions" && index === state.selectedTransactionIndex
                  return (
                    <box
                      key={transaction.source_id}
                      id={`transaction-${transaction.source_id}`}
                      style={{
                        height: 2,
                        flexDirection: "row",
                        alignItems: "center",
                        backgroundColor: selected ? "#263a32" : index % 2 === 0 ? "#111614" : "#151a18",
                      }}
                      onMouseDown={() => selectTransaction(registry, index)}
                    >
                      <text fg="#89958f" style={{ width: 11, paddingLeft: 1 }}>{transaction.date}</text>
                      <text fg="#d7dfdb" wrapMode="none" style={{ flexGrow: 1 }}>
                        {transaction.payee || "(no payee)"}
                      </text>
                      <text fg={other ? "#9fb3a9" : "#e7c66b"} style={{ width: 15 }}>
                        {short(otherLabel, 14)}
                      </text>
                      <text fg={transaction.amount < 0 ? "#ee9b90" : "#91ddb5"} style={{ width: 12 }}>
                        {money(transaction.amount)}
                      </text>
                      <text fg={transaction.pending ? "#e7c66b" : "#66736d"} style={{ width: 8 }}>
                        {transaction.pending ? "pending" : "posted"}
                      </text>
                    </box>
                  )
                })}
              </box>
            </>
          )}
        </box>
      </box>

      <box
        style={{
          height: 2,
          paddingLeft: 2,
          paddingRight: 2,
          flexDirection: "row",
          justifyContent: "space-between",
          alignItems: "center",
          backgroundColor: "#121815",
        }}
      >
        <text fg="#66736d">tab pane   ↑/↓ move   c assign   a new   p/n page</text>
        <text fg="#66736d">r refresh   s sync   q quit</text>
      </box>
    </box>
  )
}
