# Payee Resolution Prototype

This Rust module learns from historical `(imported payee, resolved payee)` examples and resolves new
imported payees with exact matching or conservative fuzzy retrieval. The server uses it for raw-payee
imports; account categorization remains separate.

Run the fixture evaluation:

```sh
cargo run --manifest-path server/Cargo.toml --bin payee_eval
```

Inspect one payee:

```sh
cargo run --manifest-path server/Cargo.toml --bin payee_eval -- \
  "SQ *JOES COFFEE #112"
```

Add `--json` for machine-readable output. Fixtures live in `examples.json`; unit tests live in
`mod.rs` beside the pure resolver implementation.

Run an interactive learning session from an empty history:

```sh
cargo run --manifest-path server/Cargo.toml --bin payee_eval -- \
  --interactive --payee Starbucks --steps 24 --seed 42
```

Interactive generation is scoped to one canonical payee for the whole session; `--payee` defaults to
`Starbucks`. Add `--seed-history` to begin with the fixture history. Before every answer, the CLI prints the noisy
imported payee, current suggestion, method, score, margin, and candidate score components. Enter a
resolved payee to add it as the next training example, press Enter to accept a suggestion, type
`skip` to leave the resolver unchanged, or `quit` to end the session.

Every interactive session writes an incrementally flushed JSONL transcript under
`.money-tui/logs/payee-eval/<session-id>.jsonl`. Use `--log /path/session.jsonl` to override it. Each
step records the generated input, complete resolver decision and candidates, raw user answer, applied
resolution, and updated history size. Normal completion adds a summary; Ctrl+C still leaves a valid
transcript through the last displayed prompt.

Add `--related` to generate meaningfully distinct names from one base payee while still applying the
same processor, store, location, and number fuzzing:

```sh
cargo run --manifest-path server/Cargo.toml --bin payee_eval -- \
  --interactive --payee Amazon --related --steps 32
```

The built-in family cycles through three identities: Amazon, Amazon Prime, and Amazon Video. Your
entered resolutions determine whether the resolver keeps these identities separate or merges them.

The resolver builds exact and fuzzy indexes once from historical examples. Exact normalized matches
use the most recent occurrence. Fuzzy matching retrieves precomputed raw-payee comparison forms
through an inverted character n-gram index and ranks them with resolved-payee-group TF-IDF cosine.
A group with one raw format must score at least `0.90`. After two distinct raw formats have resolved
to the same payee, their shared n-grams form a stable profile and use a `0.50` minimum, discounting
processor and location text that changes between examples. The winner must lead the runner-up group
by at least `0.05`; otherwise the payee remains unresolved. Numeric runs of three or more digits
become a comparison placeholder; original payee strings are never modified.

A leading Square processor prefix (`SQ *`, case-insensitive) is removed from fuzzy comparison and
from newly cleaned output names. If inference is unavailable or invalid, Monet persists a title-cased
version of the imported payee instead of leaving the raw description as the displayed payee.

During production imports, every unresolved raw payee falls back to a separate configured inference
request containing only the raw payee and its inflow/outflow direction. The structured response must
contain one nonempty `payee` string and no other fields. Valid cleaned payees are applied through
ordinary `set payee` changes and become historical examples when the next resolver snapshot is built.

Production imports process globally ordered transaction batches of 200 by default, configurable with
`MONET_TRANSACTION_BATCH_SIZE`. A new resolver snapshot is built from committed, non-pending payee
history before every batch, so local and inferred resolutions train the next batch without affecting
other transactions in their own batch.
