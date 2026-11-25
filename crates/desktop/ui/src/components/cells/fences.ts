// Owner: U1
// 171 §3.4: a reply's cells in document order, from its markdown — the ordinals `CellFence` claims and the list the
// runbook bar shows before the later fences have mounted.
import type { Cell as CellSpec } from '@/rpc/bindings/auxiliary.surface.output'
import type { CellRef } from '@/rpc/bindings/method.cells.output'
import type { Entry } from '@/store/sessions'
import { cellOf, langOf } from './grammar'

/** `end`: the index of the fence's closing line in the reply's LF-split lines (the last line when unclosed). */
export type ReplyCell = { ordinal: number; spec: CellSpec; body: string; end: number }

/** A fence body as `digest` sees it: LF line ends, one final newline trimmed. */
export const bodyKey = (body: string) => body.replaceAll('\r\n', '\n').replace(/\n$/, '')

const OPEN = /^([ \t]*(?:>[ \t]?)*[ \t]*)(`{3,}|~{3,})[ \t]*([^`\n]*)$/

/** One fenced code block: its info string, body, closing line index (the last line when unclosed) and whether it closed. */
type Fence = { info: string; body: string; end: number; closed: boolean }

/**
 * Fenced code blocks of `md` (CommonMark's rule: a closer of the same character, at least as long, nothing after it;
 * an unclosed fence runs to the end). The opener's indentation (list items) and `>` quote prefix are removed from
 * each body line.
 */
function fencesOf(md: string): Fence[] {
  const lines = md.replaceAll('\r\n', '\n').split('\n')
  const out: Fence[] = []
  for (let i = 0; i < lines.length; i++) {
    const m = OPEN.exec(lines[i] as string)
    if (!m) continue
    const [, prefix = '', fence = '', info = ''] = m
    const body: string[] = []
    let j = i + 1
    for (; j < lines.length; j++) {
      const line = (lines[j] as string).startsWith(prefix) ? (lines[j] as string).slice(prefix.length) : (lines[j] as string).trimStart()
      const close = /^[ \t]*(`{3,}|~{3,})[ \t]*$/.exec(line)
      if (close?.[1] && close[1][0] === fence[0] && close[1].length >= fence.length) break
      body.push(line)
    }
    out.push({ info, body: body.join('\n'), end: Math.min(j, lines.length - 1), closed: j < lines.length })
    i = j
  }
  return out
}

/** The cells among the fences of `md` (`fencesOf`), numbered 1… in document order. */
export function replyCells(md: string): ReplyCell[] {
  const out: ReplyCell[] = []
  for (const f of fencesOf(md)) {
    const spec = cellOf(f.info, f.body)
    if (spec) out.push({ ordinal: out.length + 1, spec, body: bodyKey(f.body), end: f.end })
  }
  return out
}

/** 219: a plain fence — closed, a runnable language (a cell language but JS/TS), no `#|` header. `ordinal` is 0-based. */
export type PlainFence = { ordinal: number; body: string }

/** 219 (Vocabulary): the plain fences of `md` in document order, numbered as `cell_promote` counts them. */
export function plainFences(md: string): PlainFence[] {
  const out: PlainFence[] = []
  for (const f of fencesOf(md)) {
    const lang = langOf(f.info)
    if (!f.closed || lang === null || lang === 'javascript' || lang === 'typescript' || cellOf(f.info, f.body) !== null) continue
    out.push({ ordinal: out.length, body: bodyKey(f.body) })
  }
  return out
}

/** Whether cell `ordinal` is the first of its reply with its `name` (the one the engine's `superseded_by` points at). */
export function firstOfName(cells: ReplyCell[], ordinal: number): boolean {
  const name = cells.find((c) => c.ordinal === ordinal)?.spec.options.name
  return !name || cells.find((c) => c.spec.options.name === name)?.ordinal === ordinal
}

const parsed = new WeakMap<Entry, ReplyCell[]>()

/** `replyCells` of an assistant entry, parsed once per entry object (so store selectors get a stable result). */
export function entryCells(e: Entry): ReplyCell[] {
  if (e.kind !== 'assistant') return []
  let out = parsed.get(e)
  if (!out) parsed.set(e, (out = replyCells(e.text)))
  return out
}

/** The transcript's cell at chat `ref` (its reply found by stored seq), `null` when the reply is not loaded. */
export function cellAt(entries: readonly Entry[], ref: CellRef): ReplyCell | null {
  if (ref.type !== 'chat') return null
  const e = entries.find((x) => x.kind === 'assistant' && x.context?.target.kind === 'message' && String(x.context.target.seq) === ref.entry)
  return (e && entryCells(e)[ref.ordinal - 1]) ?? null
}

/**
 * Item (c): the cells of reply `id` that re-present a name an earlier assistant reply of the loaded transcript
 * presented — `[ordinal, entry id of the earliest such reply]`, first cell of each name only (`firstOfName`). These
 * render as a stub pointing at the original position, which shows the newest version. A name whose earlier reply is not
 * loaded is not restated: that cell renders in full.
 */
export function restated(entries: readonly Entry[], id: string, cells: ReplyCell[]): [number, string][] {
  const names = new Map<string, number>()
  for (const c of cells) if (c.spec.options.name && !names.has(c.spec.options.name)) names.set(c.spec.options.name, c.ordinal)
  const end = names.size ? entries.findIndex((e) => e.id === id) : -1
  const out: [number, string][] = []
  for (let i = 0; i < end && names.size; i++) {
    const e = entries[i] as Entry
    for (const c of entryCells(e)) {
      const at = c.spec.options.name ? names.get(c.spec.options.name) : undefined
      if (at === undefined) continue
      out.push([at, e.id])
      names.delete(c.spec.options.name as string)
    }
  }
  return out
}
