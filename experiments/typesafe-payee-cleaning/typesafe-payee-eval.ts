import { appendFile, mkdir, writeFile } from "node:fs/promises"
import { dirname, resolve } from "node:path"
import { noul, TypeSafeClient, type NoulQuestion } from "@typesafe-ai/sdk"

type PayeePair = {
  importedPayee: string
  cleanedPayee: string
}

export type Candidate = {
  id: string
  text: string
  segmentStart: number
  segmentLength: number
}

export type ScoredCandidate = Candidate & {
  probability: number
}

type Evaluation = PayeePair & {
  predictedPayee: string
  probability: number
  exact: boolean
  tokenF1: number
  candidates: ScoredCandidate[]
  usage: {
    inputTokens: number
    outputTokens: number
  }
}

const DEFAULT_DATASET = ".money-tui/datasets/payees.jsonl"
const DEFAULT_OUTPUT = ".money-tui/evals/typesafe-payees/results.jsonl"
const DEFAULT_LIMIT = 100
const MAX_SEGMENTS = 3

export const sequentialSpans = (value: string, maximumLength = MAX_SEGMENTS): Candidate[] => {
  const segments = [...value.matchAll(/[\p{L}\p{N}]+/gu)].map((match) => ({
    start: match.index,
    end: match.index + match[0].length,
  }))
  const candidates: Candidate[] = []
  const seen = new Set<string>()
  for (let start = 0; start < segments.length; start++) {
    for (let length = 1; length <= maximumLength && start + length <= segments.length; length++) {
      const text = value.slice(segments[start].start, segments[start + length - 1].end)
      if (seen.has(text)) continue
      seen.add(text)
      candidates.push({
        id: `c${candidates.length + 1}`,
        text,
        segmentStart: start,
        segmentLength: length,
      })
    }
  }
  return candidates
}

const argument = (name: string, fallback: string) => {
  const index = process.argv.indexOf(name)
  if (index === -1) return fallback
  const value = process.argv[index + 1]
  if (!value || value.startsWith("--")) throw new Error(`${name} requires a value`)
  return value
}

const normalizedText = (value: string) =>
  value.normalize("NFKC").toLocaleLowerCase().trim().replace(/\s+/g, " ")

const tokens = (value: string) =>
  new Set([...value.normalize("NFKC").toLocaleLowerCase().matchAll(/[\p{L}\p{N}]+/gu)].map(String))

const tokenF1 = (left: string, right: string) => {
  const leftTokens = tokens(left)
  const rightTokens = tokens(right)
  if (leftTokens.size === 0 || rightTokens.size === 0) return 0
  const overlap = [...leftTokens].filter((token) => rightTokens.has(token)).length
  if (overlap === 0) return 0
  const precision = overlap / leftTokens.size
  const recall = overlap / rightTokens.size
  return (2 * precision * recall) / (precision + recall)
}

const readJsonLines = async <A>(path: string): Promise<A[]> => {
  const file = Bun.file(path)
  if (!(await file.exists())) return []
  const text = await file.text()
  return text
    .split("\n")
    .filter(Boolean)
    .map((line, index) => {
      try {
        return JSON.parse(line) as A
      } catch {
        throw new Error(`Invalid JSON on line ${index + 1} of ${path}`)
      }
    })
}

const latestDistinct = (rows: PayeePair[], limit: number) => {
  const selected: PayeePair[] = []
  const seen = new Set<string>()
  for (let index = rows.length - 1; index >= 0 && selected.length < limit; index--) {
    const row = rows[index]
    const key = normalizedText(row.importedPayee)
    if (seen.has(key)) continue
    seen.add(key)
    selected.push(row)
  }
  return selected.reverse()
}

export const selectPayeeSpan = async (client: TypeSafeClient, rawPayee: string) => {
  const candidates = sequentialSpans(rawPayee)
  if (candidates.length === 0) throw new Error(`Payee has no segments: ${rawPayee}`)
  const stateCandidates = Object.fromEntries(candidates.map((candidate) => [candidate.id, candidate.text]))
  const questions: Record<string, NoulQuestion> = Object.fromEntries(
    candidates.map((candidate) => [
      candidate.id,
      noul(
        {
          task: "Determine whether the referenced candidate is the most important merchant-identifying section of the raw financial transaction payee.",
          rawPayeePath: "state.rawPayee",
          candidatePath: `state.candidates.${candidate.id}`,
          guidance: [
            "Prefer the concise merchant, organization, person, or service identity.",
            "Exclude payment processors, transaction types, dates, locations, store numbers, card digits, reference numbers, and generic banking metadata unless they are essential to the identity.",
            "The candidate must be judged as the complete best section, not merely as text that overlaps the merchant name.",
          ],
        },
        {
          true: "This candidate is the best concise merchant-identifying section of the raw payee.",
          false: "This candidate is incomplete, includes incidental transaction metadata, or is not the merchant identity.",
        },
      ),
    ]),
  )
  const response = await client.systemOne({
    state: { rawPayee, candidates: stateCandidates },
    questions,
  })
  const scored = candidates
    .map((candidate) => ({ ...candidate, probability: response.answers[candidate.id].noul }))
    .sort(
      (left, right) =>
        right.probability - left.probability ||
        right.segmentLength - left.segmentLength ||
        left.segmentStart - right.segmentStart,
  )
  const predictedPayee = scored[0].text
  return {
    predictedPayee,
    probability: scored[0].probability,
    candidates: scored,
    usage: {
      inputTokens: response.usage.input_tokens,
      outputTokens: response.usage.output_tokens,
    },
  }
}

const evaluate = async (client: TypeSafeClient, pair: PayeePair): Promise<Evaluation> => {
  const selection = await selectPayeeSpan(client, pair.importedPayee)
  return {
    ...pair,
    ...selection,
    exact: normalizedText(selection.predictedPayee) === normalizedText(pair.cleanedPayee),
    tokenF1: tokenF1(selection.predictedPayee, pair.cleanedPayee),
  }
}

const markdownCell = (value: string) => value.replaceAll("|", "\\|").replaceAll("\n", " ")
const percent = (value: number) => `${(value * 100).toFixed(1)}%`

const writeReport = async (path: string, results: Evaluation[], target: number) => {
  const exact = results.filter((result) => result.exact).length
  const averageTokenF1 = results.reduce((sum, result) => sum + result.tokenF1, 0) / results.length
  const averageProbability =
    results.reduce((sum, result) => sum + result.probability, 0) / results.length
  const inputTokens = results.reduce((sum, result) => sum + result.usage.inputTokens, 0)
  const outputTokens = results.reduce((sum, result) => sum + result.usage.outputTokens, 0)
  const table = results
    .map(
      (result) =>
        `| ${markdownCell(result.importedPayee)} | ${markdownCell(result.cleanedPayee)} | ${markdownCell(result.predictedPayee)} | ${result.probability.toFixed(3)} | ${result.exact ? "yes" : "no"} | ${result.tokenF1.toFixed(3)} |`,
    )
    .join("\n")
  await writeFile(
    path,
    `# TypeSafe payee span evaluation

- Evaluated: ${results.length}/${target}
- Exact matches: ${exact}/${results.length} (${percent(exact / results.length)})
- Average token F1: ${averageTokenF1.toFixed(3)}
- Average selected probability: ${averageProbability.toFixed(3)}
- Token usage: ${inputTokens} input, ${outputTokens} output
- Candidate generation: sequential spans of 1–${MAX_SEGMENTS} alphanumeric segments, preserving original delimiters

| Imported payee | Current cleaned payee | TypeSafe span | P(important) | Exact | Token F1 |
|---|---|---|---:|:---:|---:|
${table}
`,
  )
}

const main = async () => {
  const datasetPath = resolve(argument("--dataset", DEFAULT_DATASET))
  const outputPath = resolve(argument("--output", DEFAULT_OUTPUT))
  const reportPath = outputPath.replace(/\.jsonl$/, ".md")
  const limit = Number.parseInt(argument("--limit", String(DEFAULT_LIMIT)), 10)
  if (!Number.isInteger(limit) || limit < 1) throw new Error("--limit must be a positive integer")

  const dataset = await readJsonLines<PayeePair>(datasetPath)
  const selected = latestDistinct(dataset, limit)
  if (selected.length < limit) throw new Error(`Dataset has only ${selected.length} distinct payees`)
  await mkdir(dirname(outputPath), { recursive: true })
  if (process.argv.includes("--force")) await writeFile(outputPath, "")
  const previous = await readJsonLines<Evaluation>(outputPath)
  const completed = new Set(
    previous.map((result) => JSON.stringify([result.importedPayee, result.cleanedPayee])),
  )
  const results = previous.filter((result) =>
    selected.some(
      (pair) =>
        pair.importedPayee === result.importedPayee && pair.cleanedPayee === result.cleanedPayee,
    ),
  )
  const client = new TypeSafeClient()

  try {
    for (const [index, pair] of selected.entries()) {
      const key = JSON.stringify([pair.importedPayee, pair.cleanedPayee])
      if (completed.has(key)) continue
      process.stdout.write(`[${index + 1}/${selected.length}] ${pair.importedPayee}\n`)
      const result = await evaluate(client, pair)
      await appendFile(outputPath, `${JSON.stringify(result)}\n`)
      results.push(result)
      completed.add(key)
      await writeReport(reportPath, results, selected.length)
    }
  } finally {
    if (results.length > 0) await writeReport(reportPath, results, selected.length)
  }
  process.stdout.write(`\nWrote ${results.length} results to ${outputPath}\nReport: ${reportPath}\n`)
}

if (import.meta.main) {
  main().catch((error) => {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`)
    process.exitCode = 1
  })
}
