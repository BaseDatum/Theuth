// 53 §4.4 / 02 §3: the `[meeting …]` user lines the daemon writes into a meeting's session — transcript
// batches, requests, taps, dismissals, deeper asks, the start line. The marker is the text (45 §7.4's
// pattern), so the transcript card recognises them by parsing, not by metadata. 57 §3.4: the oracle's
// `⚡` status row text, and which batch stamps a batch card covers (its own plus the ones the oracle skipped
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
 * One line per oracle-handled batch (57 §3.4): `⚡ topic 0.12 · skip · 78 ms`, `⚡ brief SRE-2386 0.91 · published · 81 ms`,
 * `⚡ escalate · low margin 0.55 / 0.31 · tier 2 → model`; shadow appends `✓ agreed` / `✗ model published HR-12`.
 */
export function oracleRow(d: OracleDecision): string {
  const topic = d.top.find(([l]) => l === 'topic')?.[1]
  const briefs = d.top.filter(([l]) => l !== 'topic').sort((a, b) => b[1] - a[1])
  const tier2 = d.tier === 2 ? ' · tier 2' : ''
  let s: string
  const disp = d.disposition
  switch (disp.kind) {
    case 'skip':
      s = `⚡ topic ${topic === undefined ? '?' : topic.toFixed(2)} · skip${tier2} · ${d.latency_ms} ms`
      break
    case 'brief': {
      const pick = briefs.find(([l]) => l === disp.title)
      s = `⚡ brief ${disp.title}${pick ? ` ${pick[1].toFixed(2)}` : ''} · published${tier2} · ${d.latency_ms} ms`
      break
    }
    case 'model': {
      const probs =
        briefs.length > 0 ? briefs.slice(0, 2).map(([, q]) => q.toFixed(2)).join(' / ') : topic === undefined ? '' : topic.toFixed(2)
      s = `⚡ escalate · ${disp.reason}${probs ? ` ${probs}` : ''} · tier ${d.tier} → model`
      break
    }
  }
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
