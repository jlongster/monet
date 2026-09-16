# Individual payee prompt eval

This standalone Rust package owns a copy of a payee-cleaning prompt for evaluation. It intentionally
does not import Monet's production prompt renderer, so experimental prompt versions can evolve here
without changing the application. Copy this package to create another independently versioned prompt.

From the Monet repository root, create `input.json` with one raw payee and its direction:

```json
{
  "rawPayee": "Debit Card Purchase - WAWA 8684 MIDLOTHIAN VA",
  "direction": "outflow"
}
```

Then run:

```sh
cargo payee-prompt-individual
```

The command validates `input.json` and writes the invariant agent system prompt to `prompt.md`. It
contains the cleaning instructions and required JSON output schema, while dumb-eval sends the case's
`input.json` as the user message. The command does not call an inference provider.
