// Owner: U1
// 171 §4.5 Open as note: the reply's prose and cells, each cell followed by its latest run as an ```output block and
// the run line (§4.4), created with `doc_create`. The note's cells start with no runs and their own kernel.
import type { QueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'
import { queryKeys } from '@/lib/rpc'
import type { Api } from '@/rpc/api'
import type { CellRun } from '@/rpc/bindings/CellRun'
import type { CellState } from '@/rpc/bindings/CellState'
import type { CellOrdinals } from './CellFence'
import { clock, duration, ended, errorText, exitText } from './store'

/** `cells.export_output_bytes` (§4.4): head + tail of the cleaned text. */
const EXPORT_BYTES = 8192
const HEAD = 6144

/** `ran 14:38 · exit 0 · 33 s · expect ✓` (§4.4). */
export function runLine(run: CellRun): string {
  const at = clock(run.ended_ms ?? run.queued_ms)
  const expect = run.expect.length ? ` · expect ${run.expect.every((e) => e.met) ? '✓' : '✗'}` : ''
  return `ran ${at} · ${exitText(run)}${run.secs == null ? '' : ` · ${duration(run.secs)}`}${expect}`
}

async function outputText(api: Api, run: CellRun): Promise<string> {
  const first = await api.cell_output(run.id, 0, EXPORT_BYTES, false)
  if (first.total <= EXPORT_BYTES) return first.text
  const head = first.text.slice(0, HEAD)
  const tail = await api.cell_output(run.id, first.total - (EXPORT_BYTES - HEAD), EXPORT_BYTES - HEAD, false)
  return `${head}\n… ${(first.total - EXPORT_BYTES).toLocaleString()} bytes cut …\n${tail.text}`
}

/** The note's markdown: the reply with each finished cell's output block and run line after its fence. */
export async function noteMarkdown(api: Api, ordinals: CellOrdinals, states: (CellState | null)[]): Promise<string> {
  const lines = ordinals.text.replaceAll('\r\n', '\n').split('\n')
  const after = new Map<number, string>()
  for (const [i, c] of ordinals.cells.entries()) {
    const run = states[i]?.latest
    if (!run || !ended(run)) continue
    const text = c.spec.options.private ? 'output kept from theuth (private)' : await outputText(api, run)
    const fence = '`'.repeat(Math.max(3, ...[...text.matchAll(/`{3,}/g)].map((m) => m[0].length + 1)))
    after.set(c.end, `${fence}output\n${text.replace(/\n$/, '')}\n${fence}\n${runLine(run)}`)
  }
  return lines.flatMap((l, i) => (after.has(i) ? [l, '', after.get(i) as string] : [l])).join('\n')
}

export async function openAsNote(api: Api, qc: QueryClient, ordinals: CellOrdinals, states: (CellState | null)[], open: (doc: string) => void): Promise<void> {
  try {
    const md = await noteMarkdown(api, ordinals, states)
    const title = ordinals.cells[0]?.spec.options.label ?? 'Runbook'
    const note = await api.doc_create(title, md)
    void qc.invalidateQueries({ queryKey: queryKeys.notes })
    toast('Opened as a note', { action: { label: 'Open', onClick: () => open(note.id) } })
  } catch (e) {
    toast.error(errorText(e))
  }
}
