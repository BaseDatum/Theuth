// 53 §4.4 / 02 §3: the `[meeting …]` user lines the daemon writes into a meeting's session — transcript
// batches, requests, taps, dismissals, deeper asks, the start line. The marker is the text (45 §7.4's
// pattern), so the transcript card recognises them by parsing, not by metadata. 57 §3.4: the oracle's
// status row text (drawn under a Zap icon), and which batch stamps a batch card covers (its own plus the ones the oracle skipped
// and the daemon carried into it).

import type { OracleDecision } from '@/rpc/bindings/OracleDecision'

export type MeetingLine = {
  kind: 'batch' | 'started' | 'request' | 'tapped' | 'dismissed' | 'deeper' | 'nudge' | 'summary'
  /** Chip text: `transcript 00:14:32`, `request · typed`, `tapped "KEDA"`, … */
  label: string
  /** The rest, verbatim (batch: `live:/dismissed:` header then `them:`/`me:` rows). */
  body: string
}

const STAMP = /^transcript (\d\d:\d\d:\d\d)$/
const EARLIER = /· earlier \(oracle skipped\): ([^\n]*)/

/** The `hh:mm:ss` stamps a batch card answers for: its own, then the carried ones the header names. */
export function batchStamps(line: MeetingLine): string[] {
  const own = STAMP.exec(line.label)?.[1]
  if (!own) return []
  const earlier = EARLIER.exec(line.body.split('\n')[0] ?? '')?.[1] ?? ''
  return [...earlier.split(',').map((s) => s.trim()).filter(Boolean), own]
}

/**
 * One line per oracle-handled batch (57 §3.5): the stages in order, empty ones omitted —
 * `kind: subject 0.91 · mentions: Kuiper belt, Constantin · brief: none 0.57 · tier 2: "Planet Nine" → no cached brief → model · 1442 ms`,
 * `kind: small talk 0.79 → skip · 78 ms`, `kind: subject 0.88 · brief: SRE-2386 0.91 → published · 81 ms`;
 * shadow appends `✓ agreed` / `✗ model published HR-12`.
 */
export function oracleRow(d: OracleDecision): string {
  const parts: string[] = []
  const [kind, ...rest] = d.stages
  if (kind) parts.push(`${kind[0]} ${kind[1].toFixed(2)}`)
  if (d.mentions.length > 0) parts.push(`mentions: ${d.mentions.join(', ')}`)
  for (const [label, p] of rest) parts.push(`${label} ${p.toFixed(2)}`)
  if (d.tier2 !== null) parts.push(`tier 2: ${d.tier2}`)
  const disp = d.disposition
  const went = disp.kind === 'skip' ? 'skip' : disp.kind === 'brief' ? `published ${disp.title}` : 'model'
  let s = `${parts.join(' · ')} → ${went} · ${d.latency_ms} ms`
  if (d.outcome) s += d.outcome.agreed ? ' · ✓ agreed' : d.outcome.published ? ` · ✗ model published ${d.outcome.published}` : ' · ✗ model did nothing'
  return s
}

const HEAD = /^\[meeting ([^\]]*)\]\s?/
const DISMISSED = /^\[meeting: user dismissed "([^"]*)" — not useful\]$/
const NUDGE = /^\[meeting: no brief was produced for ([\s\S]*?) — /

/** `null` for an ordinary user message. */
export function parseMeetingLine(text: string): MeetingLine | null {
  const d = DISMISSED.exec(text)
  if (d) return { kind: 'dismissed', label: `dismissed “${d[1]}”`, body: '' }
  const n = NUDGE.exec(text)
  if (n) return { kind: 'nudge', label: 'reminder: brief it', body: n[1] ?? '' }
  const m = HEAD.exec(text)
  if (!m) return null
  const head = m[1] ?? ''
  const body = text.slice(m[0].length)
  const t = /^(\d\d:\d\d:\d\d)$/.exec(head)
  if (t) {
    return body.startsWith('started.')
      ? { kind: 'started', label: 'meeting started', body: body.slice('started.'.length).trim() }
      : { kind: 'batch', label: `transcript ${t[1]}`, body }
  }
  const r = /^request, (typed|spoken)$/.exec(head)
  if (r) return { kind: 'request', label: `request · ${r[1]}`, body }
  const tap = /^tapped "([^"]*)"$/.exec(head)
  if (tap) return { kind: 'tapped', label: `tapped “${tap[1]}”`, body }
  const deep = /^deeper on "([^"]*)"/.exec(head)
  if (deep) return { kind: 'deeper', label: `deeper on “${deep[1]}”`, body }
  if (head === 'summary') return { kind: 'summary', label: 'summary requested', body }
  return null
}
