# Payee Resolution Prototype

This Rust module is isolated from the server, import pipeline, and account categorization. It learns
from historical `(imported payee, user-resolved payee)` examples and resolves new imported
payees with exact matching or conservative fuzzy retrieval.

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

The resolver uses exact minimally normalized matches, character n-gram cosine similarity,
resolved-payee-group TF-IDF, token Jaccard similarity, and recency-weighted nearest-neighbor voting.
The nearest 5 historical examples vote by `similarity² × recency`; acceptance requires lexical
similarity, support, resolved-payee agreement, and vote margin, with a near-exact exception. Fuzzy
comparison normalizes punctuation separators and replaces numeric runs of three or more digits with
a placeholder. Original payee strings are never modified.
