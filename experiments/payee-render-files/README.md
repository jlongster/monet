# Payee prompt file renderer

This experiment renders Monet's existing generative payee-cleaning prompt through files instead of
the eval runner's stdin/stdout protocol.

From the Monet repository root, create `input.json` using the `EvalInput` shape and run:

```sh
cargo payee-render-files
```

The command writes `prompt.md` in the current directory. It includes the system prompt, user prompt,
and required JSON output schema. Potential-payee hints are disabled. It only renders files and does
not call an inference provider.
