# Payee inference eval

The committed eval task is:

```text
eval/payees.eval.json
```

It defines renderer and scorer commands, opaque input and reference data, and
named variants. The current input has 100 raw payees, 70 optional potential
payees, and a gold `reference` map. No generated prompt is committed.

Prepare the eval task from the Monet repository:

```sh
bun run eval:task -- generate
# Edit potentialPayee values in eval/payees.eval.json
bun run eval:task -- label
```

`generate` snapshots the latest 100 distinct Origin payees and marks 70
`potentialPayee` fields as `EDIT ME`. `label` records reference outputs in the
same eval task without calling inference.

Select models, run the eval, and report results from the standalone runner:

```sh
cd ~/projects/eval-runner
bun run start -- models --config ../monet/eval/config.json
bun run start -- run ../monet/eval/payees.eval.json --config ../monet/eval/config.json
bun run start -- report --config ../monet/eval/config.json
```

Run one variant or override the configured model and repetitions:

```sh
bun run start -- run ../monet/eval/payees.eval.json \
  --variant no-potentials \
  --config ../monet/eval/config.json

bun run start -- run ../monet/eval/payees.eval.json \
  --model opencode/deepseek-v4-flash \
  --repetitions 3 \
  --config ../monet/eval/config.json
```

At run time eval-runner executes the task's one-shot renderer, calls the local
OpenCode `/api/generate` endpoint, and executes the one-shot scorer. Input,
reference, parameters, scores, and details remain opaque to the runner. Results
are written additively under `.money-tui/evals/runs/`.
