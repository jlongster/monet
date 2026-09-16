import { appendFile, mkdir, writeFile } from "node:fs/promises"
import { dirname, resolve } from "node:path"
import { TypeSafeClient } from "@typesafe-ai/sdk"
import { selectPayeeSpan, type ScoredCandidate } from "./typesafe-payee-eval"

type Result = {
  originalPayee: string
  cleanedPayee: string
  probability: number
  candidates: ScoredCandidate[]
  usage: {
    inputTokens: number
    outputTokens: number
  }
}

const DEFAULT_INPUT = "/mnt/mail/2026-09-15_360Checking...7327.csv"
const DEFAULT_OUTPUT = ".money-tui/evals/typesafe-bank-payees/results.jsonl"

const argument = (name: string, fallback: string) => {
  const index = process.argv.indexOf(name)
  if (index === -1) return fallback
  const value = process.argv[index + 1]
  if (!value || value.startsWith("--")) throw new Error(`${name} requires a path`)
  return value
}

export const parseCsv = (text: string) => {
  const rows: string[][] = []
  let row: string[] = []
  let field = ""
  let quoted = false
  for (let index = 0; index < text.length; index++) {
    const character = text[index]
    if (quoted) {
      if (character === '"' && text[index + 1] === '"') {
        field += '"'
        index++
      } else if (character === '"') {
        quoted = false
      } else {
        field += character
      }
    } else if (character === '"') {
      quoted = true
    } else if (character === ",") {
      row.push(field)
      field = ""
    } else if (character === "\n") {
      row.push(field.replace(/\r$/, ""))
      rows.push(row)
      row = []
      field = ""
    } else {
      field += character
    }
  }
  if (quoted) throw new Error("CSV ended inside a quoted field")
  if (field || row.length > 0) {
    row.push(field.replace(/\r$/, ""))
    rows.push(row)
  }
  return rows
}

const readJsonLines = async (path: string): Promise<Result[]> => {
  const file = Bun.file(path)
  if (!(await file.exists())) return []
  return (await file.text())
    .split("\n")
    .filter(Boolean)
    .map((line) => JSON.parse(line) as Result)
}

const markdownCell = (value: string) => value.replaceAll("|", "\\|").replaceAll("\n", " ")

const writeReport = async (path: string, results: Result[], transactionRows: number) => {
  const inputTokens = results.reduce((sum, result) => sum + result.usage.inputTokens, 0)
  const outputTokens = results.reduce((sum, result) => sum + result.usage.outputTokens, 0)
  const table = results
    .map(
      (result) =>
        `| ${markdownCell(result.originalPayee)} | ${markdownCell(result.cleanedPayee)} | ${result.probability.toFixed(3)} |`,
    )
    .join("\n")
  await writeFile(
    path,
    `# TypeSafe bank payee span selections

- CSV transaction rows: ${transactionRows}
- Distinct payees evaluated: ${results.length}
- Token usage: ${inputTokens} input, ${outputTokens} output
- Method: sequential spans of 1–3 alphanumeric segments, preserving original delimiters
- Confidence column: Noul probability that the selected span is the most important merchant-identifying section

| Original bank payee | TypeSafe-designated cleaned span | Confidence |
|---|---|---:|
${table}
`,
  )
}

const main = async () => {
  const inputPath = resolve(argument("--input", DEFAULT_INPUT))
  const outputPath = resolve(argument("--output", DEFAULT_OUTPUT))
  const reportPath = outputPath.replace(/\.jsonl$/, ".md")
  const csv = parseCsv(await Bun.file(inputPath).text())
  const headers = csv.shift()
  if (!headers) throw new Error("CSV is empty")
  const descriptionIndex = headers.indexOf("Transaction Description")
  if (descriptionIndex === -1) throw new Error("CSV has no Transaction Description column")
  const descriptions = csv.map((row) => row[descriptionIndex]?.trim()).filter(Boolean)
  const distinct = [...new Set(descriptions)]

  await mkdir(dirname(outputPath), { recursive: true })
  if (process.argv.includes("--force")) await writeFile(outputPath, "")
  const results = await readJsonLines(outputPath)
  const completed = new Set(results.map((result) => result.originalPayee))
  const client = new TypeSafeClient()

  try {
    for (const [index, originalPayee] of distinct.entries()) {
      if (completed.has(originalPayee)) continue
      process.stdout.write(`[${index + 1}/${distinct.length}] ${originalPayee}\n`)
      const selection = await selectPayeeSpan(client, originalPayee)
      const result: Result = {
        originalPayee,
        cleanedPayee: selection.predictedPayee,
        probability: selection.probability,
        candidates: selection.candidates,
        usage: selection.usage,
      }
      results.push(result)
      completed.add(originalPayee)
      await appendFile(outputPath, `${JSON.stringify(result)}\n`)
      await writeReport(reportPath, results, descriptions.length)
    }
  } finally {
    if (results.length > 0) await writeReport(reportPath, results, descriptions.length)
  }
  process.stdout.write(`\nWrote ${results.length} selections to ${outputPath}\nReport: ${reportPath}\n`)
}

if (import.meta.main) {
  main().catch((error) => {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`)
    process.exitCode = 1
  })
}
