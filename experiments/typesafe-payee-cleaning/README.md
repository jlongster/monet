# TypeSafe payee cleaning experiment

This experiment tests whether TypeSafe can select the merchant-identifying span from a raw financial
transaction payee. It splits each payee into alphanumeric segments, generates every sequential span
of one to three segments while preserving the original delimiters, and asks one TypeSafe Noul
question per candidate. The candidate with the highest probability becomes the selected payee.

Set `TYPESAFE_API_KEY` in the repository's git-ignored `.env` file before running it.

## Projected-payee evaluation

Export imported and currently cleaned payee pairs from Monet's local database:

```sh
bun run dataset:payees
```

Evaluate the latest 100 distinct imported payees against their current cleaned values:

```sh
bun run eval:typesafe-payees -- --force
```

Private inputs and results are written under `.money-tui/datasets/` and
`.money-tui/evals/typesafe-payees/`.

## Raw bank CSV evaluation

Evaluate distinct transaction descriptions from a bank CSV:

```sh
bun run eval:typesafe-bank-payees -- --input /path/to/transactions.csv --force
```

Results are written under `.money-tui/evals/typesafe-bank-payees/`. The Markdown report contains the
original description, selected span, and Noul probability; JSONL retains every candidate score.
