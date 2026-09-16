# Monet evals

This directory contains standalone evaluation packages. Prompt implementations here are intentional
copies rather than imports from the production server, so they can be changed, compared, and cloned
into new versions without affecting Monet. A validated prompt can be ported back into the application
explicitly.

- `payee-prompt-individual/` — renders the invariant system prompt used by dumb-eval's individual
  payee-cleaning cases.
