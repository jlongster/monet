import { mkdir } from "node:fs/promises";
import { createInterface } from "node:readline/promises";
import {
  EVAL_ACCOUNTS,
  addPotentialPlaceholders,
  categoryChoices,
  latestDistinctPayees,
  normalizePayee,
  type PayeeInput,
  type ReferenceResult,
} from "../eval/core";

type ProgramSpec = {
  command: string[];
  cwd?: string;
  env?: string[];
  timeoutMs?: number;
};
type EvalTask = {
  name: string;
  renderer: ProgramSpec;
  scorer: ProgramSpec;
  input: PayeeInput;
  reference: ReferenceResult;
  variants: Array<{ id: string; parameters: unknown }>;
};
type PluginConfig = {
  command: string[];
  cwd?: string;
  env?: string[];
  config?: unknown;
};
type ImportedAccount = { id: string; name: string };
type ImportedTransaction = {
  id: string;
  date: string;
  payee?: string | null;
  amount: number;
};

const ROOT = new URL("../", import.meta.url).pathname;
const EVAL_PATH = `${ROOT}eval/payees.eval.json`;
const MONET_CONFIG_PATH =
  process.env.MONET_CONFIG_PATH ??
  `${process.env.HOME}/.config/monet/config.json`;

const main = async () => {
  const command = process.argv[2];
  if (command === "generate") return generate();
  if (command === "label") return label();
  process.stdout.write(`Usage:
  bun run eval:task -- generate [--force]
  bun run eval:task -- label
`);
};

const generate = async () => {
  if (
    (await Bun.file(EVAL_PATH).exists()) &&
    !process.argv.includes("--force")
  ) {
    throw new Error(
      `Eval task already exists: eval/payees.eval.json (pass --force to replace it)`,
    );
  }
  const monet = await readJson<{ import?: PluginConfig }>(MONET_CONFIG_PATH);
  if (!monet.import)
    throw new Error(`No import plugin configured in ${MONET_CONFIG_PATH}`);
  const importer = await JsonRpcPlugin.start(monet.import);
  try {
    const initialized = await importer.request<{
      capabilities: { transactionImport: boolean };
    }>("initialize", {
      protocolVersions: [1],
      host: { name: "monet-eval", version: "0.1.0" },
      config: monet.import.config ?? {},
    });
    if (!initialized.capabilities.transactionImport)
      throw new Error("Configured plugin does not support imports");
    const accounts = await importer.request<{ accounts: ImportedAccount[] }>(
      "import/accounts",
      {},
    );
    const transactions: Array<ImportedTransaction & { accountId: string }> = [];
    for (const account of accounts.accounts) {
      process.stdout.write(`Fetching ${account.name}...\n`);
      const response = await importer.request<{
        transactions: ImportedTransaction[];
      }>("import/transactions", {
        accountId: account.id,
        initial: true,
      });
      transactions.push(
        ...response.transactions.map((transaction) => ({
          ...transaction,
          accountId: account.id,
        })),
      );
    }
    const payees = latestDistinctPayees(transactions, 100);
    if (payees.length !== 100)
      throw new Error(`Origin returned only ${payees.length} distinct payees`);
    const evalTask: EvalTask = {
      name: "payee-cleanup",
      renderer: { command: ["cargo", "payee-render"], cwd: ".." },
      scorer: { command: ["cargo", "payee-score"], cwd: ".." },
      input: {
        accounts: EVAL_ACCOUNTS,
        payees: addPotentialPlaceholders(payees),
      },
      reference: {},
      variants: [
        { id: "no-potentials", parameters: { includePotentials: false } },
        { id: "mixed-potentials", parameters: { includePotentials: true } },
      ],
    };
    await writeJson(EVAL_PATH, evalTask);
    process.stdout.write(
      `\nWrote eval/payees.eval.json with 100 payees from ${transactions.length} transactions.\n`,
    );
    process.stdout.write(
      `Edit the 70 "potentialPayee": "EDIT ME" values before labeling.\n`,
    );
  } finally {
    await importer.close();
  }
};

const label = async () => {
  const evalTask = await readJson<EvalTask>(EVAL_PATH);
  const potentialPayees = evalTask.input.payees.filter(
    (payee) => payee.potentialPayee !== undefined,
  );
  if (evalTask.input.payees.length !== 100)
    throw new Error("Eval task must contain exactly 100 payees");
  if (potentialPayees.length !== 70)
    throw new Error(`Eval task must contain exactly 70 potential payees`);
  if (potentialPayees.some((payee) => payee.potentialPayee === "EDIT ME")) {
    throw new Error(`Edit every "potentialPayee": "EDIT ME" value first`);
  }
  const categories = categoryChoices();
  const rl = createInterface({ input: process.stdin, output: process.stdout });
  try {
    for (const payee of evalTask.input.payees) {
      if (evalTask.reference[payee.id]) continue;
      const proposed = {
        payee: payee.potentialPayee ?? payee.rawPayee,
        category: null,
      };
      process.stdout.write(`[${payee.id}] Raw: ${payee.rawPayee}\n`);
      process.stdout.write(`  Direction: ${payee.direction}\n`);
      process.stdout.write(`  Potential: ${payee.potentialPayee ?? "none"}\n`);
      if (
        (await rl.question("  Label? [Enter/q] ")).trim().toLowerCase() === "q"
      ) {
        await writeJson(EVAL_PATH, evalTask);
        return;
      }
      evalTask.reference[payee.id] = await editLabel(rl, proposed, categories);
    }
  } finally {
    rl.close();
  }
  await writeJson(EVAL_PATH, evalTask);
  const correct = potentialPayees.filter(
    (payee) =>
      normalizePayee(payee.potentialPayee!) ===
      normalizePayee(evalTask.reference[payee.id].payee),
  ).length;
  process.stdout.write(
    `Reference complete. Potential mix: ${correct} correct, ${potentialPayees.length - correct} incorrect.\n`,
  );
};

const editLabel = async (
  rl: ReturnType<typeof createInterface>,
  proposed: { payee: string; category: string | null },
  categories: ReturnType<typeof categoryChoices>,
) => {
  const payee =
    (await rl.question(`  Payee [${proposed.payee}]: `)).trim() ||
    proposed.payee;
  process.stdout.write("  0. None\n  t. Transfer\n");
  categories.forEach((category, index) =>
    process.stdout.write(`  ${index + 1}. ${category.name}\n`),
  );
  const choice = (await rl.question("  Category [None]: ")).trim();
  const category =
    !choice || choice === "0"
      ? null
      : choice.toLowerCase() === "t" || choice.toLowerCase() === "transfer"
        ? "transfer"
        : (categories[Number(choice) - 1]?.alias ?? null);
  return { payee, category };
};

class JsonRpcPlugin {
  private nextId = 1;
  private pending = new Map<
    string,
    { resolve: (value: unknown) => void; reject: (error: Error) => void }
  >();
  private constructor(private child: ReturnType<typeof Bun.spawn>) {}
  static async start(config: PluginConfig) {
    const env = Object.fromEntries(
      ["PATH", "HOME", "TMPDIR", "TEMP", "TMP", ...(config.env ?? [])].flatMap(
        (name) =>
          process.env[name] === undefined ? [] : [[name, process.env[name]!]],
      ),
    );
    const child = Bun.spawn(config.command, {
      cwd: config.cwd,
      env,
      stdin: "pipe",
      stdout: "pipe",
      stderr: "pipe",
    });
    const client = new JsonRpcPlugin(child);
    void client.read();
    void new Response(child.stderr)
      .text()
      .then((text) => text.trim() && process.stderr.write(text));
    return client;
  }
  request<A>(method: string, params: unknown) {
    const id = String(this.nextId++);
    const promise = new Promise<A>((resolve, reject) =>
      this.pending.set(id, {
        resolve: resolve as (value: unknown) => void,
        reject,
      }),
    );
    this.child.stdin.write(
      `${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`,
    );
    this.child.stdin.flush();
    return promise;
  }
  async close() {
    if (this.child.exitCode !== null) return;
    await Promise.race([
      this.request("shutdown", {}).catch(() => {}),
      Bun.sleep(1_000),
    ]);
    this.child.stdin.end();
    if (this.child.exitCode === null) this.child.kill();
    await this.child.exited;
  }
  private async read() {
    const reader = this.child.stdout.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    while (true) {
      const { done, value } = await reader.read();
      buffer += decoder.decode(value, { stream: !done });
      const lines = buffer.split("\n");
      buffer = lines.pop() ?? "";
      for (const line of lines) {
        if (!line) continue;
        const message = JSON.parse(line);
        const pending = this.pending.get(String(message.id));
        if (!pending) continue;
        this.pending.delete(String(message.id));
        if (message.error) pending.reject(new Error(message.error.message));
        else pending.resolve(message.result);
      }
      if (done) break;
    }
  }
}

const readJson = async <A>(path: string) => (await Bun.file(path).json()) as A;
const writeJson = async (path: string, value: unknown) => {
  await mkdir(new URL(".", `file://${path}`).pathname, { recursive: true });
  await Bun.write(path, `${JSON.stringify(value, null, 2)}\n`);
};

main().catch((error) => {
  process.stderr.write(
    `${error instanceof Error ? error.message : String(error)}\n`,
  );
  process.exitCode = 1;
});
