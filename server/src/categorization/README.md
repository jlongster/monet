# Categorization Pipeline Prototype

This Rust module assumes payee resolution has completed and predicts an income or expense counterpart
account from confirmed historical transactions. After payee resolution, production imports assign
opening balances, reconcile transfers involving every touched transaction, then build one frozen
categorization snapshot for the entire sync and commit accepted counterparts as ordinary changes.

Categorized pending transactions remain eligible history. If a posted transaction replaces one, its
stable source identity and changes carry forward through the normal pending-replacement flow.

Run fixture evaluations:

```sh
cargo run --manifest-path server/Cargo.toml --bin categorization_eval
```

Run an interactive learning session:

```sh
cargo run --manifest-path server/Cargo.toml --bin categorization_eval -- \
  --interactive \
  --payee "Starbucks,Target,Kroger" \
  --imported-category coffee \
  --amount -7.50 \
  --source-type asset \
  --steps 24
```

`--payee` accepts one exact resolved payee or a comma-delimited list sampled by the generator. The CLI
does not generate fuzzy payee variants and does not run or prompt for payee resolution. Enter a listed
counterpart account ID or type a new name such as `food`; unknown values create session-local category
accounts and immediately become training labels. Press Enter to accept a suggestion, type `skip`, or
`quit`. Add `--seed-history` to start from category fixture history.

Generation is seeded and reproducible with `--seed 42`. Without overrides, the CLI uses Amazon,
random $10-$100 outflows, mixed source types, recent dates, and matching sample source accounts.
Configure every scored field with:

```text
--payee Amazon,Target
--amount 12.99
--amount-profile default|small|large|mixed
--direction inflow|outflow|mixed
--source-type asset|liability|mixed
--source-account checking,savings,credit-card
--date 2026-08-01
--date-profile recent|old|mixed
--seed 42
```

`small` generates $10-$15, `large` generates $1,000-$1,200, `mixed` randomly uses either band, and
`default` generates $10-$100. Fixed `--amount` overrides its profile. `--date` sets the first date;
every subsequent transaction advances by a seeded positive number of days. `recent` and `old` advance
by 1-7 days, while `mixed` advances by 7-28 days to span a wider historical range. Generated runs are
always strictly chronological.

Every interactive session writes an incrementally flushed JSONL transcript under
`.money-tui/logs/categorization-eval/<session-id>.jsonl`. Use `--log /path/session.jsonl` to override
it. The transcript records generation settings, every query, raw user input, the suggested and
applied categories, whether the suggestion was accepted, any newly
proposed category, session-local account creation, and the final summary. Neighbor scores and other
resolution internals are intentionally omitted because fixtures assert category behavior only.

After every interactive run, the CLI asks whether to create a regression fixture. Answer `y` and
enter a fixture name to write it under `server/src/categorization/fixtures/runs/`; the CLI immediately
replays it, reports any differences, and prints the exact command needed to regenerate the run.

Fixtures preserve starting accounts and history, every generated query, its suggested category,
user-applied training decisions, and the canonical rerun command.

The normal Rust test suite automatically replays every stored run. Every step asserts only that its
suggested category, including `UNCATEGORIZED`, is unchanged. User decisions reconstruct training
history but never define expectations. Scores, neighbors, reasons, and floating-point details do not
fail fixtures. Failures identify the fixture, step, transaction context, expected suggestion, and
actual suggestion.

The pipeline applies:

1. Exact normalized resolved-payee history only; no fuzzy payee matching.
2. Future-dated history is excluded, then amount direction must match exactly.
3. Amounts are sorted with the incoming amount. If the largest adjacent gap is at least 10x, only
   history on the incoming amount's side of that gap remains; without a 10x gap, amount is ignored.
4. Remaining history receives one flat score: 80% date recency, 15% source account type, and 5%
   specific source account. Date decays gradually with a 90-day half-life starting on day one.
5. The five highest scores vote directly, without squaring. One distinct category wins immediately;
   multiple categories require at least 65% weighted vote share and two supporting transactions.
6. A validated category suggested by the existing payee-cleaning LLM response is used only when no
   usable local cohort exists. Ambiguous local history always remains uncategorized.
7. Reconciled counterparts outside income/expense are represented as one generic transfer outcome.
   The most recent exact-payee transaction being a transfer bails immediately; otherwise transfer
   examples vote normally and only a winning transfer outcome bails.

For a newly inserted transaction, either a local transfer outcome or LLM `category: "transfer"`
leaves it uncategorized and writes the one-time effective note `Potential transfer`. Later syncs never
rewrite that note, so user edits remain authoritative.

The categorizer does not make a separate LLM request. Only transactions already sent through payee
fallback can carry an LLM category suggestion. Confident local history takes precedence, and accepted
counterparts become history for the next sync. If exact payee history exists but remains ambiguous
across categories, Monet leaves the transaction uncategorized and does not use the LLM suggestion.
