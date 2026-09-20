// 53 §4.4 / 02 §3: the `[meeting …]` user lines the daemon writes into a meeting's session — transcript
// batches, requests, taps, dismissals, deeper asks, the start line. The marker is the text (45 §7.4's
// pattern), so the transcript card recognises them by parsing, not by metadata.

export type MeetingLine = {
  kind: 'batch' | 'started' | 'request' | 'tapped' | 'dismissed' | 'deeper' | 'nudge' | 'summary'
  /** Chip text: `transcript 00:14:32`, `request · typed`, `tapped "KEDA"`, … */
  label: string
  /** The rest, verbatim (batch: `live:/dismissed:` header then `them:`/`me:` rows). */
  body: string
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
