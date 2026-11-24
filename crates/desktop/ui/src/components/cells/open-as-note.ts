// Owner: U1
// 171 §4.5 Open as note: the reply's prose and cells, each cell followed by its latest run as an ```output block (none
// when the run printed nothing) and the run line (§4.4), created with `doc_create`. The note's cells start with no
// runs and their own kernel; a cell's first run in the note replaces that history (`historyAfter`, 171F4 T2-3).
import type { QueryClient } from '@tanstack/react-query'
import { notify as toast } from '@/lib/notify'
import { queryKeys } from '@/lib/rpc'
import type { Api } from '@/rpc/bindings/client'
import type { CellRun } from "@/rpc/bindings/method.cells.output"
import type { CellState } from "@/rpc/bindings/method.cells.output"
import type { CellOrdinals } from './CellFence'
import { clock, ended, errorText } from './store'

/** `cells.export_output_bytes` (§4.4): head + tail of the cleaned text. */
const EXPORT_BYTES = 8192
const HEAD = 6144

/**
 * `ran 14:38 · exit 0 · 33 s · expect ✓` (§4.4), character for character the engine's `doc_read` run line
 * (`crates/cells/src/note.rs` `run_line`): the end time to the minute, `exit` + code/signal/`timeout`/…, seconds to
 * one decimal under 10 s.
 */
export function runLine(run: CellRun): string {
  const t = run.ended_ms ?? run.started_ms
  const exit = ['timeout', 'killed', 'lost', 'cancelled'].includes(run.state) ? run.state : (run.exit?.value ?? '?')
  // ponytail: toFixed rounds an exact binary tie (0.25 s) up where Rust's `{:.1}` rounds it to even; port Rust's rule if a tie ever shows.
  const secs = run.secs == null ? '' : ` · ${run.secs.toFixed(run.secs < 10 ? 1 : 0)} s`
  const expect = run.expect.length ? ` · expect ${run.expect.every((e) => e.met) ? '✓' : '✗'}` : ''
  return `ran ${t == null ? '—' : clock(t)} · exit ${exit}${secs}${expect}`
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
    const text = (c.spec.options.private ? 'output kept from theuth (private)' : await outputText(api, run)).replace(/\n+$/, '')
    const fence = '`'.repeat(Math.max(3, ...[...text.matchAll(/`{3,}/g)].map((m) => m[0].length + 1)))
    after.set(c.end, text.trim() ? `${fence}output\n${text}\n${fence}\n${runLine(run)}` : runLine(run))
  }
  return lines.flatMap((l, i) => (after.has(i) ? [l, '', after.get(i) as string] : [l])).join('\n')
}

/** The slice of a BlockNote block `historyAfter` reads. */
type NoteBlock = { id: string; type: string; props?: { language?: string }; content?: unknown; children?: NoteBlock[] }

/**
 * 171F4 T2-3: the ids of the history Open as note wrote right after cell block `id` (an `output` code block, then the
 * `ran …` paragraph; either may be alone), searched at any depth; `[]` when none follows. A cell's own run replaces it,
 * so the note shows one output per cell, the newest.
 */
export function historyAfter(blocks: NoteBlock[], id: string): string[] {
  const at = blocks.findIndex((b) => b.id === id)
  if (at < 0) return blocks.flatMap((b) => historyAfter(b.children ?? [], id))
  const out: string[] = []
  let next = blocks[at + 1]
  if (next?.type === 'codeBlock' && next.props?.language === 'output') {
    out.push(next.id)
    next = blocks[at + 2]
  }
  const text = next?.type === 'paragraph' && Array.isArray(next.content) ? next.content.map((c: { text?: string }) => c.text ?? '').join('') : ''
  if (next && /^ran (\d\d:\d\d|—) · /.test(text)) out.push(next.id)
  return out
}

/** Presses of Open as note in flight, by `session/entry`: a double click makes one note. */
const inflight = new Set<string>()

/**
 * 171F B9: one note per reply (`cell_reply_note_get/set`, keyed by session and entry). A second press opens the note
 * the first made; a note since trashed or deleted is made again and replaces the remembered one.
 */
export async function openAsNote(api: Api, qc: QueryClient, session: string, ordinals: CellOrdinals, states: (CellState | null)[], open: (doc: string) => void): Promise<void> {
  const entry = ordinals.entry
  const key = `${session}/${entry}`
  if (!entry || inflight.has(key)) return
  inflight.add(key)
  try {
    const known = await api.cell_reply_note_get(session, entry)
    // limit: the 500 newest notes (the notes list's own window); an older remembered note is made again.
    if (known && (await api.doc_list('note', 500)).some((d) => d.id === known)) return open(known)
    const md = await noteMarkdown(api, ordinals, states)
    const title = ordinals.cells[0]?.spec.options.label ?? 'Runbook'
    const note = await api.doc_create(title, md)
    await api.cell_reply_note_set(session, entry, note.id)
    void qc.invalidateQueries({ queryKey: queryKeys.notes })
    open(note.id)
  } catch (e) {
    toast.error(errorText(e))
  } finally {
    inflight.delete(key)
  }
}
