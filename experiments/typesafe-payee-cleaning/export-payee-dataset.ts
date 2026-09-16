import { mkdir } from "node:fs/promises"
import { dirname, resolve } from "node:path"
import { Database } from "bun:sqlite"

type PayeePair = {
  importedPayee: string
  cleanedPayee: string
}

const argument = (name: string, fallback: string) => {
  const index = process.argv.indexOf(name)
  if (index === -1) return fallback
  const value = process.argv[index + 1]
  if (!value || value.startsWith("--")) throw new Error(`${name} requires a path`)
  return value
}

const databasePath = resolve(argument("--db", ".money-tui/money.db"))
const outputPath = resolve(argument("--output", ".money-tui/datasets/payees.jsonl"))
const database = new Database(databasePath, { readonly: true, strict: true })

try {
  const rows = database
    .query<PayeePair, []>(`
      SELECT
        source.payee AS importedPayee,
        MIN(posting.payee) AS cleanedPayee
      FROM transactions_source AS source
      JOIN postings AS posting ON posting.source_id = source.id
      WHERE source.payee IS NOT NULL
        AND TRIM(source.payee) <> ''
        AND posting.payee IS NOT NULL
        AND TRIM(posting.payee) <> ''
      GROUP BY source.id, source.payee
      ORDER BY source.date, source.id
    `)
    .all()

  await mkdir(dirname(outputPath), { recursive: true })
  await Bun.write(outputPath, rows.map((row) => JSON.stringify(row)).join("\n") + "\n")

  const normalize = (value: string) => value.trim().toLocaleLowerCase().replace(/\s+/g, " ")
  const changed = rows.filter(
    (row) => normalize(row.importedPayee) !== normalize(row.cleanedPayee),
  ).length
  const distinctPairs = new Set(
    rows.map((row) => JSON.stringify([row.importedPayee, row.cleanedPayee])),
  ).size

  process.stdout.write(
    `Wrote ${rows.length} payee pairs (${distinctPairs} distinct, ${changed} cleaned) to ${outputPath}\n`,
  )
} finally {
  database.close()
}
