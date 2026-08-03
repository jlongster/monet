# Architecture Handoff

This document records the product direction, implemented prototype, research conclusions, and
current finance data-model recommendation as of July 4, 2026. The next session should continue
from this document before changing the database or application architecture.

## Product Intent

Money TUI is a local personal-finance application. Double-entry accounting should be its core
model, but routine UI language should be accounts, categories, income, spending, and moving money.
The user should not need to think in debits and credits.

All durable application data should live in SQLite. The interface should remain minimal and
well-structured, use borders selectively, develop a visual identity distinct from conventional
terminal dashboards, and eventually support themes through semantic design tokens.

The immediate architecture discussion has deliberately narrowed to the finance domain and data
model. Import UX, budgeting, reports, and broader product questions should wait until the ledger
model is settled.

## Repository State

The repository is currently a prototype, not a finance implementation. There is no SQLite schema,
migration system, repository layer, or accounting domain code yet. The displayed balances and
accounts in `src/app.tsx` are static data.

The entire repository currently appears as untracked in Git. A future session must not assume that
the existing files have been committed.

Current commands:

```sh
bun install
bun run dev
bun run test
bun run typecheck
```

## Terminal Control

Monet exposes read-only UI semantics through `@kitlangton/terminal-control-opentui` when launched
under Terminal Control. The provider is initialized after the OpenTUI renderer is created.

```sh
termctrl run monet --host opentui --cwd "$PWD" -- bun run dev
```

## Finance Model Direction

The internal model should be a general ledger with arbitrary balanced postings. It should not use
a pair-only source/destination schema because split transactions naturally involve more than two
accounts.

User-facing bank, cash, and credit-card records are ledger accounts. User-facing categories are
income and expense accounts internally. Equity accounts and import-clearing details can remain
hidden from ordinary UI.

Examples using signed posting amounts:

```text
Coffee purchase
Assets:Checking              -5.00
Expenses:Dining               5.00
```

```text
Salary
Assets:Checking            3,000.00
Income:Salary             -3,000.00
```

```text
Transfer
Assets:Checking             -500.00
Assets:Savings               500.00
```

```text
Credit-card purchase
Liabilities:CreditCard       -75.00
Expenses:Groceries            75.00
```

The sum of every finalized journal revision must be zero. The UI should translate accounting sign
conventions where necessary, such as displaying a negative liability ledger balance as a positive
amount owed.

## Versioned Ledger Recommendation

The current recommendation is a versioned ledger. It is intended to preserve auditability without
making event replay the source of all application state.

```text
journal_entries
    id
    current_revision_id
    source
    created_at

journal_entry_revisions
    id
    journal_entry_id
    revision
    date
    payee
    description
    status
    created_at

postings
    id
    revision_id
    account_id
    amount_minor
    currency
```

A journal entry provides stable identity. Editing it creates a new immutable revision and a new
balanced posting set. `current_revision_id` selects the version used by current balances and
reports. Earlier revisions remain available for history.

Every revision must independently balance. A partially written or unbalanced revision must never
become current. Creating a revision, validating its postings, and changing `current_revision_id`
must happen inside one SQLite transaction.

This recommendation has been discussed but not implemented or finally accepted as schema.

## Event Sourcing Clarification

The exact architecture review from the current discussion is preserved here:

"Fully append-only event-sourced" means the database never stores the current transaction directly.
It stores events such as:

```text
TransactionCreated
TransactionCategorized
TransactionAmountChanged
TransactionVoided
```

The current ledger is reconstructed by replaying those events. This adds considerable complexity:
projections, event versioning, replay logic, and migrations.

That is different from normal double-entry bookkeeping.

For example, correcting Coffee from Groceries to Dining could work three ways.

Mutable ledger:

```text
UPDATE postings SET account_id = dining
```

This is simple, but loses history.

Strict append-only accounting:

```text
Reverse original transaction
Create corrected transaction
```

This is auditable, but routine recategorization creates noisy reversal entries.

Versioned ledger:

```text
Journal entry keeps a stable identity
Each edit creates a balanced revision
Current revision is used for balances
Older revisions remain available for history
```

The recommendation is the third option.

The proposed correction policy is:

| State | Correction behavior |
| --- | --- |
| Draft or pending | Editable through new revisions |
| Posted but unreconciled | Recategorization and corrections create new revisions |
| Reconciled | Locked against ordinary editing |
| Reconciled correction | Explicit reversal or adjustment entry |

The original statement meant "avoid replaying an event stream as the source of all state," not
"discard auditability."

## Proposed Core Tables

The following table set is a working proposal rather than an implemented schema.

| Table | Purpose |
| --- | --- |
| `accounts` | Asset, liability, income, expense, and equity accounts |
| `journal_entries` | Stable identity and current revision pointer |
| `journal_entry_revisions` | Immutable versions of transaction-level fields |
| `postings` | Balanced account movements belonging to one revision |
| `payees` | Normalized merchant or payer identity |
| `import_batches` | One file or provider synchronization operation |
| `import_records` | Immutable source rows exactly as imported |
| `import_matches` | Links source rows to canonical entries and postings |
| `balance_assertions` | External statement balances used for reconciliation |
| `categorization_rules` | Future repeatable classification behavior |

`accounts.kind` should distinguish `asset`, `liability`, `income`, `expense`, and `equity`.
Categories should not be duplicated into an unrelated category model; they should be income or
expense accounts presented through category-oriented UI and APIs.

Amounts must not use SQLite floating point. The initial recommendation is signed integer minor
units such as `1234` for `$12.34`, with an explicit currency code. This is straightforward for a
single-currency first version. Correct multi-currency and investment accounting would require
cost, price, or valuation semantics and remains unresolved.

Balances should always be derived from current-revision postings. A mutable cached balance must
not become authoritative.

## Imports And Uncategorized Money

Imported source records should be immutable and separate from canonical journal entries. Editing a
payee or category must never alter the source data used for deduplication.

An imported checking-account outflow can initially balance against a hidden uncategorized expense
account:

```text
Assets:Checking                 -45.00
Expenses:Uncategorized          45.00
```

An imported inflow can initially balance against an uncategorized income account:

```text
Assets:Checking                 900.00
Income:Uncategorized           -900.00
```

Categorization creates a new journal revision whose counterpart posting uses the selected income
or expense account. It should not rewrite the immutable imported amount or source identity.

Transfers imported from both owned accounts must eventually map to one canonical journal entry,
with both source records linked to it. Automatic transfer matching is important but should be
designed after the core ledger and revision schema.

## SQLite Integrity

SQLite should be configured with foreign keys enabled and use WAL plus a busy timeout:

```sql
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 5000;
```

Cross-row balancing cannot be expressed as a simple `CHECK` constraint. All ledger writes should
go through one domain service that creates a complete revision, verifies the sum by currency, and
atomically makes it current. Database triggers or finalization constraints may be added as defense
in depth, but they should not replace domain validation and invariant tests.

The UI should call domain services rather than writing ledger SQL
directly.

## Research Conclusions

Firefly III demonstrates the benefit of presenting transactions as source and destination money
movement, but its transaction-group and paired-journal structure becomes complex for splits.

GnuCash demonstrates the correct arbitrary-split transaction model and posting-level reconciliation,
but exposes more accounting terminology than this product should.

Beancount provides the strongest guiding invariant: every transaction balances, metadata remains
attached to ledger objects, and balance assertions compare the ledger against reality.

hledger demonstrates useful import behavior: dry runs, reusable mapping rules, idempotency, and
explicit unknown income or expense accounts.

Actual Budget demonstrates why imports need stable imported IDs, reconciliation, and careful
matching when both sides of a transfer arrive independently.

Lunch Money demonstrates a friendly review workflow where users categorize imported transactions
and may turn repeated decisions into rules.

## Decisions Still Open

The next discussion should remain focused and settle these core points before implementation:

| Decision | Current recommendation |
| --- | --- |
| Canonical correction model | Stable journal identity with immutable balanced revisions |
| Category representation | Income and expense accounts surfaced as categories |
| Initial numeric scope | Integer minor units and one practical currency |
| Finalized balance invariant | Sum of postings is zero per supported currency model |
| Reconciled corrections | Explicit reversal or adjustment, not ordinary revision |
| Imported source data | Immutable records linked to canonical journal entries |

The best next step is to challenge and finalize the versioned-ledger shape, particularly whether
`current_revision_id` is acceptable and exactly when an entry becomes locked. Only then should the
first SQLite migration and domain service be written.
