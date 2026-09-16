import Papa from 'papaparse'

/** A parsed CSV/TSV: header row, up to `max` body rows, and the total body row count (35 §3.2). */
export type Table = { header: string[]; rows: string[][]; total: number }

/** RFC 4180 parse (quoted fields, embedded newlines); `tsv` forces `\t`, else the delimiter is sniffed. The first row is the header. */
export function parseCsv(text: string, tsv: boolean, max = 500): Table {
  const { data } = Papa.parse<string[]>(text, { delimiter: tsv ? '\t' : undefined, skipEmptyLines: true })
  const [header = [], ...body] = data
  return { header, rows: body.slice(0, max), total: body.length }
}
