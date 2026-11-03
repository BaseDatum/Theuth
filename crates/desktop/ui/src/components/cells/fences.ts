// Owner: U1
// 171 §3.4: a reply's cells in document order, from its markdown — the ordinals `CellFence` claims and the list the
// runbook bar shows before the later fences have mounted.
import type { CellSpec } from '@/rpc/bindings/CellSpec'
import { cellOf } from './grammar'

/** `end`: the index of the fence's closing line in the reply's LF-split lines (the last line when unclosed). */
export type ReplyCell = { ordinal: number; spec: CellSpec; body: string; end: number }

/** A fence body as `digest` sees it: LF line ends, one final newline trimmed. */
export const bodyKey = (body: string) => body.replaceAll('\r\n', '\n').replace(/\n$/, '')

const OPEN = /^([ \t]*(?:>[ \t]?)*[ \t]*)(`{3,}|~{3,})[ \t]*([^`\n]*)$/

/**
 * Fenced code blocks of `md` (CommonMark's rule: a closer of the same character, at least as long, nothing after it;
 * an unclosed fence runs to the end). The opener's indentation (list items) and `>` quote prefix are removed from
 * each body line. Returns the cells among them, numbered 1… in document order.
 */
export function replyCells(md: string): ReplyCell[] {
  const lines = md.replaceAll('\r\n', '\n').split('\n')
  const out: ReplyCell[] = []
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
    i = j
    const text = body.join('\n')
    const spec = cellOf(info, text)
    if (spec) out.push({ ordinal: out.length + 1, spec, body: bodyKey(text), end: Math.min(j, lines.length - 1) })
  }
  return out
}
