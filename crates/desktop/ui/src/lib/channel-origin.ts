// 172 D4/D5: where a user row came from (Slack, Signal, Telegram) and the Slack shared-message blocks in its text.
import type { ChannelOrigin } from '@/rpc/bindings/ChannelOrigin'

/** The 110 §7.1 marker line Slack puts first for the model (`crates/slack/src/channel.rs`, `handle_message`). */
const MARKER = /^\[slack (dm|channel)\](?:\n|$)/

/** The row's typed origin; for a row stored before 172, the one its leading Slack marker line implies. */
export function originOf(entry: { text: string; channel?: ChannelOrigin }): ChannelOrigin | undefined {
  if (entry.channel) return entry.channel
  const m = MARKER.exec(entry.text)
  return m ? { via: 'slack', chat: m[1] === 'dm' ? 'dm' : 'group' } : undefined
}

/** The text without one leading `[slack dm]` / `[slack channel]` line; anything else unchanged. */
export const stripMarker = (text: string): string => text.replace(MARKER, '')

/** A one-line preview of a user text, or a title seeded from one: without the Slack marker; a marker-only text (the
 * engine seeds a Slack session's title from its first line) as its chip label. */
export function previewText(text: string): string {
  const origin = originOf({ text })
  return stripMarker(text).trim() || (origin ? originLabel(origin) : text)
}

/** A search snippet (ranges in scalars, `MatchRange`) without its leading Slack marker: ranges shift left, a range inside
 * the marker goes. A snippet cut mid-text starts with `…` and is unchanged. */
export function stripSnippet<R extends { start: number; end: number }>(text: string, ranges: R[]): { text: string; ranges: R[] } {
  const cut = text.length - stripMarker(text).length
  if (cut === 0) return { text, ranges }
  return {
    text: text.slice(cut),
    ranges: ranges.filter((r) => r.end > cut).map((r) => ({ ...r, start: Math.max(0, r.start - cut), end: r.end - cut })),
  }
}

/** The chip label (172 D4): `Slack · #general`, `Signal`, `Telegram · DM · from ops`. */
export function originLabel(o: ChannelOrigin): string {
  const where =
    o.via === 'slack'
      ? `Slack · ${o.chat === 'dm' ? 'DM' : o.title ? `#${o.title}` : 'channel'}`
      : o.via === 'signal'
        ? 'Signal'
        : `Telegram · ${o.chat === 'dm' ? 'DM' : o.title || 'group'}`
  return o.from ? `${where} · from ${o.from}` : where
}

/** A forwarded or linked Slack message (`forward.rs` `shared`): header fields, the unquoted body, any `(files: …)`. */
export type SharedMessage = {
  kind: 'forwarded' | 'linked'
  author: string
  channel?: string
  time?: string
  permalink?: string
  body: string
  files?: string
}

export type Segment =
  | { kind: 'text'; text: string }
  | { kind: 'shared'; message: SharedMessage }
  | { kind: 'attachment'; from?: string; text: string }

const HEAD = /^\[(forwarded|linked) message from (.+)\]$/
const TIME = /^\d{4}-\d{2}-\d{2} \d{2}:\d{2} UTC$/
const SOURCE = /^\((?:channel|ts|thread_ts) [^\s,)]+(?:, (?:ts|thread_ts) [^\s,)]+)*\)$/
const FILES = /^\(files: (.+)\)$/
const ATTACHMENT = /^\[attachment(?: from ([^\]]+))?\](?: (.*))?$/

/** `forwarded message from A · #chan · time · url` → its fields, in `shared`'s order; `null` off-grammar. */
function header(line: string): Omit<SharedMessage, 'body' | 'files'> | null {
  const m = HEAD.exec(line)
  if (!m) return null
  const [author = '', ...rest] = (m[2] ?? '').split(' · ')
  const out: Omit<SharedMessage, 'body' | 'files'> = { kind: m[1] === 'forwarded' ? 'forwarded' : 'linked', author }
  // Each optional field at most once, in order: `#chan`, time, permalink.
  let at = 0
  for (const part of rest) {
    if (at < 1 && part.startsWith('#') && part.length > 1) [out.channel, at] = [part.slice(1), 1]
    else if (at < 2 && TIME.test(part)) [out.time, at] = [part, 2]
    else if (at < 3 && /^https?:\/\/\S+$/.test(part)) [out.permalink, at] = [part, 3]
    else return null
  }
  return author ? out : null
}

/**
 * The body split into the user's own words and Slack's shared blocks (`crates/slack/src/forward.rs`):
 * a `[forwarded|linked message from …]` header, its `> ` lines (none for an empty body), then optional
 * `(files: …)` and `(channel …, ts …)` lines (the source line is dropped); an `[attachment( from X)] text`
 * line. Anything the grammar does not admit stays text.
 */
export function parseShared(text: string): Segment[] {
  const lines = text.split('\n')
  const out: Segment[] = []
  let plain: string[] = []
  const flush = () => {
    if (plain.length) out.push({ kind: 'text', text: plain.join('\n') })
    plain = []
  }
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i] ?? ''
    const head = header(line)
    if (head) {
      let j = i + 1
      const quoted: string[] = []
      for (let q = lines[j]; q !== undefined && /^>(?: |$)/.test(q); q = lines[++j]) quoted.push(q.slice(2))
      const files = FILES.exec(lines[j] ?? '')?.[1]
      if (files !== undefined) j++
      if (SOURCE.test(lines[j] ?? '')) j++
      flush()
      out.push({ kind: 'shared', message: { ...head, body: quoted.join('\n'), ...(files ? { files } : {}) } })
      i = j - 1
      continue
    }
    const a = ATTACHMENT.exec(line)
    if (a) {
      flush()
      out.push({ kind: 'attachment', ...(a[1] ? { from: a[1] } : {}), text: a[2] ?? '' })
      continue
    }
    plain.push(line)
  }
  flush()
  return out
}
