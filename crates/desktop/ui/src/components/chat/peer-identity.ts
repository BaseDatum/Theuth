import { useQuery } from '@tanstack/react-query'
import { useExports, useContacts } from '@/lib/contacts'
import { engineLabel } from '@/lib/owner'
import { queryKeys, useApi } from '@/lib/rpc'
import { useMesh } from '@/components/tasks/queries'
import type { A2aPeerStatus } from '@/rpc/bindings/A2aPeerStatus'
import type { Contact } from '@/rpc/bindings/Contact'
import type { MeshStatus } from '@/rpc/bindings/MeshStatus'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import type { AgentInfo } from '@/rpc/extra'
import { useSessionField } from '@/store/sessions'

/** Who a peer card or a send names: words for the header, never a raw uuid when the UI knows better. */
export type Sender = {
  name: string
  subtitle?: string
  /** A chat the UI can open (`/c/$session`): a subagent, the parent or another of the owner's sessions. */
  session?: string
  /** The stable identity the circle colour hashes: the address as the engine wrote it. */
  key: string
  /** A job's or a reminder's note: one fixed neutral circle (an icon, never a hashed colour). */
  kind?: 'job' | 'reminder'
}

/** Job notes (`job j4` live; `j4 · cmd · exit 0 · 3s` and `j4 idle 63s` stored) and reminders (`reminder r3` live,
 * `Reminder r3 · after 10 min` stored), told apart from the label `from`. */
export function labelKind(from: string): Sender['kind'] {
  if (/^(job j\w+|j\d+ (·|idle\b))/.test(from)) return 'job'
  if (/^reminder r\w+/i.test(from)) return 'reminder'
  return undefined
}

/** What `resolveSender` reads: the open chat, its subtree and the caches the rest of the app already keeps. */
export type Directory = {
  self: string
  children?: Record<string, AgentInfo>
  sessions?: readonly SessionSummary[]
  /** `a2a_status().peers`: maps a `peer:<binding uuid>` sender to its alias. */
  peers?: readonly Pick<A2aPeerStatus, 'name' | 'binding'>[]
  contacts?: readonly Pick<Contact, 'name' | 'machine'>[]
  mesh?: Pick<MeshStatus, 'self_id' | 'self_name' | 'peers'>
  /** The export this chat is published as, for `via <export>` on an incoming contact message. */
  exported?: string
}

/** The short id a uuid falls back to. */
export const shortId = (id: string): string => id.slice(0, 8)

/** An engine-authenticated endpoint (resolved through the caches), as opposed to a label like `job j4` or `Reminder r3`. */
export const isAddress = (from: string): boolean => /^(session|peer|contact):/.test(from) || from === 'parent'

/** A contact's or an A2A peer's words: plain text only. */
export const isOutside = (from: string): boolean => /^(contact|peer):/.test(from)

function machineOf(row: SessionSummary, mesh: Directory['mesh']): string | undefined {
  if (!mesh) return undefined
  if (row.here || !row.owner) return mesh.self_name
  return engineLabel(row.owner, [{ id: mesh.self_id, name: mesh.self_name }, ...mesh.peers])
}

/**
 * The sender's name and subtitle. Name preference: a subagent's name, a session's title, a contact's name, the alias,
 * then the 8-character id. Subtitle: `Your subagent · <harness>`, `Parent`, `Chat on <machine>`, or
 * `Contact · on <machine> · via <export>` (each part only when known).
 */
export function resolveSender(address: string, dir: Directory): Sender {
  const key = address
  const selfRow = dir.sessions?.find((s) => s.id === dir.self)
  const sessionId = address === 'parent' ? (selfRow?.parent ?? undefined) : address.startsWith('session:') ? address.slice('session:'.length) : undefined
  if (address === 'parent' || sessionId !== undefined) {
    const id = sessionId ?? ''
    const child = id ? dir.children?.[id] : undefined
    const row = id ? dir.sessions?.find((s) => s.id === id) : undefined
    const parent = !!id && selfRow?.parent === id
    // A subagent's row carries its peer name; its title is only the brief's first line.
    const name = child?.name || (row?.parent ? row.name : null) || row?.title || row?.name || (id ? shortId(id) : 'Parent')
    const subtitle = child
      ? `Your subagent · ${child.agent ?? 'theuth'}`
      : parent || address === 'parent'
        ? 'Parent'
        : row
          ? (() => {
              const m = machineOf(row, dir.mesh)
              return m ? `Chat on ${m}` : 'Chat'
            })()
          : undefined
    return { name, subtitle, key, session: child || row ? id : undefined }
  }
  if (address.startsWith('contact:')) {
    // `contact:<holding engine>:<contact name>`: the holder vouches for the name. Only this engine's own contact list
    // describes a contact it holds; another machine's contact is never matched against it.
    const rest = address.slice('contact:'.length)
    const colon = rest.indexOf(':')
    // A colon-less address names no holder: the whole rest is the name, and no local contact is assumed.
    const holder = colon < 0 ? '' : rest.slice(0, colon)
    const alias = colon < 0 ? rest : rest.slice(colon + 1)
    if (holder && dir.mesh && holder === dir.mesh.self_id) return contactSender(alias, undefined, dir, key)
    // Never a raw engine id: a holder the (loaded) mesh does not name is "another machine".
    const on = dir.mesh?.peers.find((p) => p.id === holder)?.name ?? 'another machine'
    return { name: alias, subtitle: ['Contact', `held on ${on}`, dir.exported && `via ${dir.exported}`].filter(Boolean).join(' · '), key }
  }
  if (address.startsWith('peer:')) {
    // `peer:<binding uuid>` inbound, `peer:<alias>` or `peer:<alias>/<export>` outbound.
    const rest = address.slice('peer:'.length)
    const slash = rest.indexOf('/')
    const head = slash < 0 ? rest : rest.slice(0, slash)
    const exp = slash < 0 ? undefined : rest.slice(slash + 1)
    const alias = dir.peers?.find((p) => p.binding === head)?.name ?? head
    return contactSender(alias, exp, dir, key)
  }
  // A label (`job j4`, `Reminder r3`, a legacy wrapper's `from`): a subagent by name when it is one.
  const kind = labelKind(address)
  if (kind) return { name: address, key, kind }
  const child = Object.values(dir.children ?? {}).find((c) => c.name === address)
  return child ? { name: child.name, subtitle: `Your subagent · ${child.agent ?? 'theuth'}`, key, session: child.id } : { name: address, key }
}

/** How a send names one recipient: the sender name, plus the export of a `peer:<alias>/<export>` address (`sky · hi-there`). */
export function recipientLabel(address: string, dir: Directory): string {
  const exp = address.startsWith('peer:') ? address.split('/')[1] : undefined
  return `${resolveSender(address, dir).name}${exp ? ` · ${exp}` : ''}`
}

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

function contactSender(alias: string, exp: string | undefined, dir: Directory, key: string): Sender {
  const contact = dir.contacts?.find((c) => c.name === alias)
  const via = exp ?? dir.exported
  if (contact) {
    const parts = ['Contact', contact.machine && `on ${contact.machine}`, via && `via ${via}`].filter(Boolean)
    return { name: contact.name, subtitle: parts.join(' · '), key }
  }
  // An unknown binding is only an id; a configured A2A peer keeps its alias.
  if (UUID.test(alias)) return { name: shortId(alias), subtitle: 'Outside agent', key }
  return { name: alias, subtitle: via ? `A2A peer · via ${via}` : 'A2A peer', key }
}

/** Circle text: a 1–2 character first word as written (`Z4` for `z4-183`), else its first letter. */
export function initials(name: string): string {
  const word = name.trim().split(/[\s\-_·./:]+/)[0] ?? ''
  return (word.length <= 2 ? word : word.slice(0, 1)).toUpperCase() || '?'
}

/** The project palette minus ochre and carnelian (warning and danger) and umber, which reads as ochre in the dark theme.
 * Classes are safelisted in styles.css. */
const CIRCLES = ['lapis', 'faience', 'malachite', 'plum', 'sand', 'slate', 'nile'] as const

/** One stable `bg-project-<key>` per sender identity (FNV-1a of the address). */
export function senderColor(key: string): string {
  let h = 0x811c9dc5
  for (let i = 0; i < key.length; i++) h = Math.imul(h ^ key.charCodeAt(i), 0x01000193)
  return `bg-project-${CIRCLES[(h >>> 0) % CIRCLES.length]}`
}

/** The directory for chat `self`, from the caches the app already keeps. `outside`: also read the contact and A2A lists. */
export function useDirectory(self: string, outside: boolean): Directory {
  const api = useApi()
  const children = useSessionField(self, 'children')
  const sessions = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() }).data
  const mesh = useMesh().data
  const peers = useQuery({ queryKey: queryKeys.a2a, queryFn: () => api.a2a_status(), enabled: outside }).data?.peers
  const contacts = useContacts(outside).data
  const exported = useExports(outside).data?.find((x) => x.target.kind === 'chat' && x.target.session === self)?.name
  return { self, children, sessions, peers, contacts, mesh, exported }
}

// ------------------------------------------------------------------------------------------------ sends and receipts

/** One recipient's row of a `send_message` result. */
export type SentReceipt = { id: string; to: string; state: string; error?: string }

/** A refusal before anything was accepted carries the nil id: nothing to look up. */
export const NIL = '00000000-0000-0000-0000-000000000000'

const UUID_SRC = '[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}'
// peers.rs `receipt_line`: `<to> <state> id=<uuid>`, then ` task=<t>` and ` error=<e>` when set; either may hold spaces.
// `s`: JavaScript's `.` stops at U+2028/U+2029, which a remote task, error or machine name may carry; the output is
// already split on `\n`, so dotAll cannot reach the next line.
const RECEIPT = new RegExp(`^(\\S+) (accepted|consumed|replied|suppressed|failed|unknown|held) id=(${UUID_SRC})(?: task=.*?)?(?: error=(.*))?$`, 's')
// The current engine's reply block header; an older engine has none.
const HEADER = new RegExp(`^reply id=(${UUID_SRC}):$`)

/** `parseReceipts`: the receipts, the reply text by receipt id, and whether the receipt block stopped at a line that
 * is neither a reply header, nor the end, nor the recipient cap (an older engine's reply cut a fan-out short). */
export type ParsedSend = { receipts: SentReceipt[]; replies: Record<string, string>; unexpected: boolean }

/**
 * `send_message`'s text output, from either engine. Both start with the LEADING block of receipt lines, read up to
 * `max` (the call's recipient count) and never past the first other line. Then:
 * - current engines: `reply id=<uuid>:` header lines, each followed by that receipt's reply text up to the next
 *   header; a block whose id is not a `replied` receipt of the block is ignored, and the first block for an id wins;
 * - older engines: the reply sits right under its receipt, so the rest is a reply only when the LAST receipt is
 *   `replied` (never under failed, held or any other state).
 * Reply text is the other side's: it is never parsed for receipts. With several receipts, the card prefers each
 * fetched record's reply (a fake header inside one reply could otherwise move text between recipients).
 */
export function parseReceipts(output: string, max: number): ParsedSend {
  const lines = output.split('\n')
  const receipts: SentReceipt[] = []
  let i = 0
  for (; i < lines.length && receipts.length < max; i++) {
    const m = RECEIPT.exec(lines[i] as string)
    if (!m) break
    receipts.push({ to: m[1] as string, state: m[2] as string, id: m[3] as string, ...(m[4] !== undefined && { error: m[4] }) })
  }
  const replies: Record<string, string> = {}
  // Only a `replied` receipt has a reply, in either format.
  const ids = new Set(receipts.filter((r) => r.state === 'replied').map((r) => r.id))
  if (i < lines.length && HEADER.test(lines[i] as string)) {
    let id: string | undefined
    let body: string[] = []
    const flush = () => {
      if (id && ids.has(id) && !(id in replies)) replies[id] = body.join('\n')
    }
    for (; i < lines.length; i++) {
      const h = HEADER.exec(lines[i] as string)
      if (!h) {
        body.push(lines[i] as string)
        continue
      }
      flush()
      id = h[1]
      body = []
    }
    flush()
    return { receipts, replies, unexpected: false }
  }
  const last = receipts[receipts.length - 1]
  if (i < lines.length && last?.state === 'replied') replies[last.id] = lines.slice(i).join('\n')
  return { receipts, replies, unexpected: receipts.length > 0 && i < lines.length && receipts.length < max }
}

/** No further transition is possible. */
export const isTerminal = (state: string): boolean => state === 'replied' || state === 'failed' || state === 'suppressed'

/** Where a send stands: `step` 1–3 lights Sent, Read, Answered; `tone` colours the chip and the line under the track. */
export type Track = { step: 0 | 1 | 2 | 3; tone: 'ok' | 'info' | 'warning' | 'danger' | 'muted'; word: string; note?: string }

/** Engine reason words a receipt's `error` may carry, in words. */
const REASONS: Record<string, string> = { interrupted_by_restart: 'Interrupted by a restart' }

/** Receipt state → the delivery track (`state` is a string so `held` type-checks before and after the bindings name it). */
export function trackOf(state: string, error?: string | null): Track {
  const why = error ? (REASONS[error] ?? error) : undefined
  switch (state) {
    case 'accepted':
      return { step: 1, tone: 'info', word: 'sent' }
    case 'consumed':
      return { step: 2, tone: 'info', word: 'read' }
    case 'replied':
      return { step: 3, tone: 'ok', word: 'answered' }
    case 'held':
      return { step: 1, tone: 'warning', word: 'held', note: why || 'Held by the other side until they allow it.' }
    case 'failed':
      return { step: 0, tone: 'danger', word: 'not delivered', note: why || 'Not delivered.' }
    case 'suppressed':
      return { step: 0, tone: 'danger', word: 'suppressed', note: why || 'Suppressed.' }
    case 'unknown':
      return { step: 1, tone: 'muted', word: 'outcome unknown', note: why }
    default:
      return { step: 0, tone: 'muted', word: state }
  }
}

// ------------------------------------------------------------------------------------------------------ repeats

const norm = (s: string): string => s.trim().replace(/\s+/g, ' ')

/** The same words after trimming and collapsing whitespace. */
export const sameText = (a: string, b: string): boolean => norm(a) === norm(b)

/** A one-line preview: the first non-blank line without Markdown emphasis, code ticks, or heading/list/quote markers. */
export function firstLine(text: string): string {
  const line = text
    .split('\n')
    .map((l) => l.trim())
    .find(Boolean)
  return (line ?? '').replace(/^(#{1,6}\s+|[-*+]\s+|>\s*|\d+\.\s+)/, '').replace(/\*\*|__|`/g, '')
}
