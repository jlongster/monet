# Payee prompt renderer

This standalone CLI renders Monet's generative payee-cleaning prompt through files. It lives outside
the server executable while reusing the shared `monet_server::payee_prompt` library module.

From the Monet repository root, create `input.json` using the `EvalInput` shape and run:

```sh
cargo payee-prompt-render
```

The command writes `prompt.md` in the current directory. It includes the system prompt, user prompt,
and required JSON output schema. Potential-payee hints are disabled. It only renders files and does
not call an inference provider.
