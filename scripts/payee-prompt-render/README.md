# Payee prompt renderer

This standalone CLI renders Monet's generative payee-cleaning prompt through files. It lives outside
the server executable while reusing the shared `monet_server::payee_prompt` library module.

From the Monet repository root, create `input.json` with one raw payee and its direction:

```json
{
  "rawPayee": "Debit Card Purchase - WAWA 8684 MIDLOTHIAN VA",
  "direction": "outflow"
}
```

Then run:

```sh
cargo payee-prompt-render
```

The command writes the invariant agent system prompt to `prompt.md`. It contains the cleaning
instructions and required JSON output schema, while dumb-eval sends the case's `input.json` as the
user message. The command does not call an inference provider.
