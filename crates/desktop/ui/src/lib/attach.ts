// 66 §3: a message's attachments are references — `{kind, id, title, meta}` — serialised as leading marker lines
// of the user text (`[attached <kind> <id> "<title>" · <meta>]`, one per line, then the body). This module is the
// one grammar (`marker`/`parse`, mirrored by `agent::attach` in Rust with the same fixtures) and the one naming
// table (§8: `title`/`subtitle` over the record a picker row was built from), so the chip, the row, the history
// chip and the line the model reads all carry the same words.

import type { DocSummary } from "@/rpc/bindings/method.agent.output"
import type { BlockHit } from "@/rpc/bindings/method.document.output"
import type { HistorySearchHit } from "@/rpc/bindings/method.rpc_2dcompose.output"
import type { MeetingSummary } from "@/rpc/bindings/method.meeting.output"
import type { Meta } from "@/rpc/bindings/method.knowledge.output"
import type { SearchHit } from "@/rpc/bindings/method.rpc_2dcompose.output"
import type { SessionSummary } from "@/rpc/bindings/method.agent.output"
import type { Artifact } from "@/rpc/bindings/method.artifact.output"

export type Kind = 'note' | 'knowledge' | 'artifact' | 'session' | 'meeting' | 'notebook' | 'source' | 'whiteboard'

export const KINDS: readonly Kind[] = ['note', 'knowledge', 'artifact', 'session', 'meeting', 'notebook', 'source', 'whiteboard']

export type AttachRef = {
  kind: Kind
  /** DocId | concept id[@commit] | artifact call_id | SessionId | MeetingId — what the kind's read tool accepts. */
  id: string
  /** What the user saw (§8); never empty. */
  title: string
  /** `pdf v2 · 3 pages · 84 KB · Sep 21` — display-only, may be `""`. */
  meta: string
}

/** The user-facing word per kind (§12 q2: "Chat" in the UI, `session` on the wire). */
export const KIND_LABEL: Record<Kind, string> = {
  note: 'Note',
  knowledge: 'Knowledge',
  artifact: 'Artifact',
  session: 'Chat',
  meeting: 'Meeting',
  notebook: 'Notebook',
  source: 'Source',
  whiteboard: 'Whiteboard',
}

/** Identity for dedupe: the same thing attached twice is one chip. */
export const key = (r: { kind: Kind; id: string }): string => `${r.kind}:${r.id}`

export const isKind = (s: string): s is Kind => (KINDS as readonly string[]).includes(s)

/** `\x` → `x` for any `x`: the writer only escapes `"` and `\`, the reader accepts any pair the grammar admits. */
const unescape = (s: string) => s.replace(/\\(.)/g, '$1')

/** The marker line (§3.3): `[attached <kind> <id> "<title>" · <meta>]`; ` · <meta>` only when `meta` is non-empty. */
export function marker(ref: AttachRef): string {
  const title = ref.title.replace(/\\/g, '\\\\').replace(/"/g, '\\"')
  return `[attached ${ref.kind} ${ref.id} "${title}"${ref.meta ? ` · ${ref.meta}` : ''}]`
}

const ATTACHED = /^\[attached (note|knowledge|artifact|session|meeting|notebook|source|whiteboard) (\S+) "((?:[^"\\]|\\.)*)"(?: · ([^\]]*))?\]$/
/** 59 §5.4's spelling: `[doc <DocId> "<title>"]` is a note, `[doc knowledge:<id> "<title>"]` a knowledge concept. */
const LEGACY = /^\[doc (\S+) "((?:[^"\\]|\\.)*)"\]$/

/** One line as a reference, or `null` when it is not a well-formed marker (then it is body text). */
export function parseLine(line: string): AttachRef | null {
  const m = ATTACHED.exec(line)
  if (m) return { kind: m[1] as Kind, id: m[2] ?? '', title: unescape(m[3] ?? ''), meta: m[4] ?? '' }
  const l = LEGACY.exec(line)
  if (!l) return null
  const id = l[1] ?? ''
  const title = unescape(l[2] ?? '')
  return id.startsWith('knowledge:')
    ? { kind: 'knowledge', id: id.slice('knowledge:'.length), title, meta: '' }
    : { kind: 'note', id, title, meta: '' }
}

/**
 * Split a user text into its leading marker lines and the body: parsing stops at the first line that is not a
 * marker, so a bracketed line after the first sentence is never misread. `parse(marker(r) + '\n' + body).body === body`.
 */
export function parse(text: string): { refs: AttachRef[]; body: string } {
  const lines = text.split('\n')
  const refs: AttachRef[] = []
  let i = 0
  for (; i < lines.length; i++) {
    const ref = parseLine(lines[i] ?? '')
    if (!ref) break
    refs.push(ref)
  }
  return { refs, body: lines.slice(i).join('\n') }
}

/** The leading marker lines, then the body — what the client sends (§4) and what `EditBubble` re-prepends. */
export function compose(refs: readonly AttachRef[], body: string): string {
  return refs.length > 0 ? `${refs.map(marker).join('\n')}\n${body}` : body
}

/** 218 §1: a whiteboard's marker meta is its card count — `1 card`, `5 cards`. */
export const cardsLabel = (cards: number): string => `${cards} ${cards === 1 ? 'card' : 'cards'}`

/** `[attached whiteboard <DocId> "<title>" · N cards]` as a reference. */
export function whiteboardRef(doc: string, title: string, cards: number): AttachRef {
  return { kind: 'whiteboard', id: doc, title: title || 'Untitled whiteboard', meta: cardsLabel(cards) }
}

// ---- §8 naming --------------------------------------------------------------------------------------------------

/** An artifact row from either shape the UI sees: `list_artifacts` (numbers) or a search hit (the ts-rs binding, bigints). */
export type ArtifactRecord = Omit<Artifact, 'bytes' | 'created_at'> & { bytes: number | bigint; created_at: number | bigint }

/** The record a picker row was built from; `title`/`subtitle`/`toRef` read the same table from it. */
export type Source =
  | { kind: 'note'; doc: DocSummary }
  | { kind: 'knowledge'; meta: Meta }
  | { kind: 'knowledge'; revision: HistorySearchHit }
  | { kind: 'artifact'; artifact: ArtifactRecord; sessionTitle?: string }
  | { kind: 'session'; session: SessionSummary }
  | { kind: 'session'; hit: SearchHit }
  | { kind: 'meeting'; meeting: MeetingSummary }
  | { kind: 'notebook'; hit: BlockHit }
  | { kind: 'source'; hit: BlockHit }
  /** `cards` when the caller knows the count (the marker's meta), else the row reads `whiteboard · updated …`. */
  | { kind: 'whiteboard'; doc: DocSummary; cards?: number }

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec']

/** Local `MMM d` this year, `MMM d, yyyy` otherwise. */
export function date(at: Date | number | string, now: Date = new Date()): string {
  const d = at instanceof Date ? at : new Date(at)
  if (Number.isNaN(d.getTime())) return ''
  const s = `${MONTHS[d.getMonth()]} ${d.getDate()}`
  return d.getFullYear() === now.getFullYear() ? s : `${s}, ${d.getFullYear()}`
}

/** `KB`/`MB` with one decimal below 10 (`1.2 KB`, `84 KB`, `3.5 MB`); a whole number drops its `.0`. */
export function size(bytes: number | bigint): string {
  const n = Number(bytes)
  if (n < 1024) return `${n} B`
  const fmt = (v: number, unit: string) => `${v < 10 ? v.toFixed(1).replace(/\.0$/, '') : Math.round(v)} ${unit}`
  return n < 1024 * 1024 ? fmt(n / 1024, 'KB') : fmt(n / (1024 * 1024), 'MB')
}

/** A chat's name: its title, else `Untitled chat · <created MMM d>` — the engine's own rule for search hits (§8). */
/**
 * 183 R2: a title seeded from a peer message's wrapper (`<message from="…" …>`, before the engine named those
 * chats `Message from …`) reads `Message from <who>[: <first body line>]`, never the wrapper; one seeded from a
 * schedule fire's wrapper (`<schedule name="…" at="…" …>`) reads as the schedule's name. Anything else as is.
 */
export function cleanTitle(title: string): string {
  const t = title.trim()
  const sched = /^<schedule name="([^"]*?)…?(?:"|$)/.exec(t)?.[1]?.trim()
  if (sched) return sched
  const m = /^<message from="([^"]*)/.exec(t)
  if (!m) return title
  const from = m[1] ?? ''
  const who = from.startsWith('peer:') && from.length > 5 ? from.slice(5) : 'another agent'
  const gt = t.indexOf('>')
  const first = gt < 0 ? undefined : t.slice(gt + 1).split('</message>')[0]!.split('\n').map((l) => l.trim()).find(Boolean)
  return first ? `Message from ${who}: ${first}` : `Message from ${who}`
}

export const chatName = (s: { title: string; created_at: string }, now: Date = new Date()): string =>
  cleanTitle(s.title) || `Untitled chat · ${date(s.created_at, now)}`

/** The title the user sees, with §8's fallback when the record's is empty; never `""`. */
export function title(src: Source, now: Date = new Date()): string {
  switch (src.kind) {
    case 'note':
      return src.doc.title || `Untitled note · ${date(Number(src.doc.created_ms), now)}`
    case 'knowledge':
      return 'meta' in src ? src.meta.title || src.meta.id : src.revision.title || src.revision.id
    case 'artifact':
      return src.artifact.title || src.artifact.name
    case 'session':
      return chatName('hit' in src ? src.hit : src.session, now)
    case 'meeting':
      return src.meeting.title || src.meeting.process_name || `Meeting · ${date(src.meeting.started_at, now)}`
    case 'notebook':
    case 'source':
      return src.hit.title || `Untitled ${src.kind}`
    case 'whiteboard':
      return src.doc.title || 'Untitled whiteboard'
  }
}

/** The muted second line of a row and the marker's `meta`. */
export function subtitle(src: Source, now: Date = new Date()): string {
  switch (src.kind) {
    case 'note':
      return `note · updated ${date(Number(src.doc.updated_ms), now)} · ${size(src.doc.bytes)}`
    case 'knowledge':
      if ('meta' in src)
        return [src.meta.type, src.meta.id, src.meta.timestamp ? date(src.meta.timestamp, now) : ''].filter(Boolean).join(' · ')
      return `revision ${src.revision.commit.slice(0, 7)} · ${date(src.revision.time, now)}`
    case 'artifact': {
      const a = src.artifact
      const parts = [a.kind, `v${a.version}`]
      if (a.pages !== null) parts.push(`${a.pages} pages`)
      parts.push(size(a.bytes), date(Number(a.created_at) * 1000, now))
      if (src.sessionTitle) parts.push(`in "${src.sessionTitle}"`)
      return parts.join(' · ')
    }
    case 'session': {
      if ('hit' in src) return `chat · updated ${date(src.hit.updated_at, now)}${src.hit.archived ? ' · archived' : ''}`
      const s = src.session
      return `chat · ${s.project ?? 'no project'} · updated ${date(s.updated_at, now)}${s.archived ? ' · archived' : ''}`
    }
    case 'meeting': {
      const m = src.meeting
      const parts = [date(m.started_at, now)]
      if (m.duration_ms !== null) parts.push(`${Math.max(1, Math.round(Number(m.duration_ms) / 60_000))} min`)
      if (m.state !== 'Done') parts.push(typeof m.state === 'string' ? m.state.toLowerCase() : 'processing')
      return parts.join(' · ')
    }
    case 'notebook':
    case 'source':
      return [src.kind, src.hit.heading, src.hit.block ? `#${src.hit.block}` : ''].filter(Boolean).join(' · ')
    case 'whiteboard':
      return src.cards !== undefined
        ? cardsLabel(src.cards)
        : `whiteboard · updated ${date(Number(src.doc.updated_ms), now)}`
  }
}

/** The stable identity the kind's read tool accepts. */
export function idOf(src: Source): string {
  switch (src.kind) {
    case 'note':
      return src.doc.id
    case 'knowledge':
      return 'meta' in src ? src.meta.id : `${src.revision.id}@${src.revision.commit}`
    case 'artifact':
      return src.artifact.call_id
    case 'session':
      return 'hit' in src ? src.hit.session : src.session.id
    case 'meeting':
      return src.meeting.id
    case 'notebook':
    case 'source':
      return src.hit.doc
    case 'whiteboard':
      return src.doc.id
  }
}

/** The reference a row attaches: id, and the title/meta the user is looking at, frozen (§12 q12). */
export function toRef(src: Source, now: Date = new Date()): AttachRef {
  return { kind: src.kind, id: idOf(src), title: title(src, now), meta: subtitle(src, now) }
}

// ---- Cross-component events ------------------------------------------------------------------------------------

/** `theuth:attach`: put `ref` in `session`'s composer as a chip (`null` = the new-chat draft). */
export type AttachRequest = { session: string | null; ref: AttachRef }

export function attachTo(session: string | null, ref: AttachRef): void {
  window.dispatchEvent(new CustomEvent<AttachRequest>('theuth:attach', { detail: { session, ref } }))
}

/** `theuth:attach-open`: open `session`'s picker (§7.1). */
export function openAttachPicker(session: string | null): void {
  window.dispatchEvent(new CustomEvent<{ session: string | null }>('theuth:attach-open', { detail: { session } }))
}

/** Drag payload type: a row dragged onto a composer attaches (§7.8). */
export const DRAG_TYPE = 'application/x-theuth-attach'

/** The ref a `DRAG_TYPE` drop carries, `null` when the drop is something else (files) or unreadable. */
export function droppedRef(dt: DataTransfer): AttachRef | null {
  const raw = dt.getData(DRAG_TYPE)
  if (!raw) return null
  try {
    const r = JSON.parse(raw) as Partial<AttachRef>
    return KINDS.includes(r.kind as Kind) && typeof r.id === 'string' && r.id !== ''
      ? { kind: r.kind as Kind, id: r.id, title: r.title || r.id, meta: r.meta ?? '' }
      : null
  } catch {
    return null
  }
}

/** `current` plus the `added` refs it lacks (by kind + id), order kept. */
export function addRefs(current: readonly AttachRef[], added: readonly AttachRef[]): AttachRef[] {
  const seen = new Set(current.map(key))
  const out = [...current]
  for (const r of added) {
    if (!seen.has(key(r))) {
      seen.add(key(r))
      out.push(r)
    }
  }
  return out
}

// ---- `@` trigger (§7.8) --------------------------------------------------------------------------------------------

/**
 * The `@word` the caret sits in when `@` starts a word (start of text or after whitespace): `{start, word}` with
 * `start` the index of the `@`; `null` otherwise (`a@b`, `sky@x.com`). The word is left as typed unless a row is picked.
 */
export function atTrigger(text: string, caret: number): { start: number; word: string } | null {
  const before = text.slice(0, caret)
  const m = /(^|\s)@(\S*)$/.exec(before)
  if (!m) return null
  const word = m[2] ?? ''
  return { start: before.length - word.length - 1, word }
}
