import { useQuery } from '@tanstack/react-query'
import { queryKeys, useApi } from '@/lib/rpc'
import type { Contact } from '@/rpc/bindings/Contact'
import type { Export } from '@/rpc/bindings/Export'
import type { Risk } from '@/rpc/bindings/Risk'
import type { ShareMode } from '@/rpc/bindings/ShareMode'

/** §12.7: the cap as a contact row and a card say it. `destructive` is never offered (§7.6). */
export const CAP: Record<Risk, string> = { read_only: 'read only', mutating: 'can change things', destructive: 'destructive' }

/** 183 W-27: what each first-contact answer does, one sentence per button (the Attention screen and the HUD row). */
export const FIRST_ANSWERS = 'Allow once lets this message through. Always lets every later message to this export through. Block blocks this contact entirely.'

/** 186: what a contact reaches, in one line (Contacts, Contacts' access): only what is shared, in the mode chosen each time. */
export const SHARE_RULE =
  'A contact reaches only what you share, and you choose how each time. Full collaboration: their agent and yours talk freely, with Markdown, links and images, and yours works with its normal tools and your usual approvals. Cautious: plain text, one limited turn per message, read-only tools.'

/** 186: the pairing dialog's line: pairing alone shares nothing. */
export const PAIRED_RULE =
  'A paired contact reaches nothing until you share an agent or chat with them, and you choose how: Full collaboration (their agent and yours talk freely) or Cautious (plain text, one limited turn per message, read-only tools).'

/** 186 D1: a grant's mode; an export with no entry is cautious (D2: fail closed). */
export const modeOf = (c: Pick<Contact, 'modes'>, exp: string): ShareMode => c.modes[exp] ?? 'cautious'

/** 186 D6: the cap bounds cautious shares only: it means something for a contact with no share yet or a cautious one. */
export const capBinds = (c: Pick<Contact, 'exports' | 'modes'>): boolean => !c.exports.length || c.exports.some((n) => modeOf(c, n) === 'cautious')

/** 186: `<Mode>: <hint>` once per distinct mode of `modes`, Full first (the allow strip, the Attention screen). */
export const modeLines = (modes: readonly ShareMode[], kind: 'agent' | 'chat' = 'chat'): string[] =>
  (['full', 'cautious'] as const).filter((m) => modes.includes(m)).map((m) => `${MODE_LABEL[m]}: ${modeHint(m, kind)}`)

/** 186: each mode's name and what it lets through, said alike by the share buttons, the mode selects and the strip. */
export const MODE_LABEL: Record<ShareMode, string> = { full: 'Full collaboration', cautious: 'Cautious' }
export const modeHint = (mode: ShareMode, kind: 'agent' | 'chat' = 'chat'): string =>
  mode === 'full'
    ? `Their agent and ${kind === 'chat' ? 'this chat’s agent' : 'this agent'} talk freely, with Markdown, links and images, and ${kind === 'chat' ? 'this chat’s agent' : 'it'} works with its normal tools.`
    : 'Plain text, one limited turn per message, read-only tools.'
/** 186 D10: why Full collaboration is off for a contact on an older build. */
export const updateNeeded = (c: Pick<Contact, 'name' | 'machine'>) => `update ${c.machine || c.name} to share in full`

/** `contact_list()`: this machine's contacts, refreshed by `A2aChanged` and a finished ceremony. */
export function useContacts(enabled = true) {
  const api = useApi()
  return useQuery({ queryKey: queryKeys.contacts, queryFn: () => api.contact_list(), enabled })
}

/** `a2a_exports()`: this machine's exports (agents and chats), with `paused`. */
export function useExports(enabled = true) {
  const api = useApi()
  return useQuery({ queryKey: queryKeys.exports, queryFn: () => api.a2a_exports(), enabled })
}

/** `contact_first_waiting()`: the (contact, export) pairs whose first message waits for the owner; live on `A2aChanged`. */
export function useFirstWaiting(enabled = true) {
  const api = useApi()
  return useQuery({ queryKey: queryKeys.waiting, queryFn: () => api.contact_first_waiting(), enabled })
}

/** 183 §12.10: an exported chat — its export and the contacts it is granted to (blocked ones excluded). */
export type Sharing = { exp: Export; contacts: Contact[] }

/** The `Sharing` of chat `session`; `null` when the chat is not exported. */
export function sharingOf(session: string, exports: readonly Export[] | undefined, contacts: readonly Contact[] | undefined): Sharing | null {
  const exp = exports?.find((x) => x.target.kind === 'chat' && x.target.session === session)
  return exp ? { exp, contacts: (contacts ?? []).filter((c) => !c.blocked && c.exports.includes(exp.name)) } : null
}

export function useSharing(session: string | null, enabled = true): Sharing | null {
  const exports = useExports(enabled && !!session)
  const contacts = useContacts(enabled && !!session)
  return session ? sharingOf(session, exports.data, contacts.data) : null
}

/** 183 §12.10: the chat header's chip — `Shared with bob · read only`, each contact with its cap (186: `· full` for a full
 * share, whose turns the cap does not bound), `· paused (chat archived)` when the export answers nothing; `null` when no
 * contact can message the chat. */
export function sharedLabel(s: Sharing | null): string | null {
  if (!s?.contacts.length) return null
  return `Shared with ${s.contacts.map((c) => `${c.name} · ${modeOf(c, s.exp.name) === 'full' ? 'full' : CAP[c.cap]}`).join(', ')}${s.exp.paused ? ` · paused (${s.exp.paused})` : ''}`
}

/** `Stop sharing with bob` / `with bob and alice` / `with bob, alice and carol`. */
export function names(cs: readonly Pick<Contact, 'name'>[]): string {
  const n = cs.map((c) => c.name)
  return n.length < 2 ? (n[0] ?? '') : `${n.slice(0, -1).join(', ')} and ${n[n.length - 1]}`
}

/** A `[a-z0-9_-]` export name (the `peers.yaml` key) from a title or agent, unique among `taken`. */
export function exportName(title: string, taken: readonly string[]): string {
  const base =
    title
      .toLowerCase()
      .replace(/[^a-z0-9_-]+/g, '-')
      .replace(/^-+/, '')
      .slice(0, 32)
      .replace(/-+$/, '') || 'export'
  let name = base
  for (let n = 2; taken.includes(name); n++) name = `${base}-${n}`
  return name
}

/** §7.3: the endpoint id in base32 (RFC 4648, as the engine's `Contact.fingerprint`), first 20 characters in groups of 4.
 * Anything but a 64-hex endpoint id (a TXT hint) is grouped as written. */
export function fingerprint(eid: string): string {
  let b32 = eid
  if (/^[0-9a-f]{64}$/.test(eid)) {
    b32 = ''
    let bits = 0
    let acc = 0
    for (const h of eid.slice(0, 26)) {
      acc = (acc << 4) | parseInt(h, 16)
      bits += 4
      if (bits >= 5) {
        bits -= 5
        b32 += 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567'[(acc >> bits) & 31]
        acc &= (1 << bits) - 1
      }
    }
  }
  return (b32.slice(0, 20).match(/.{1,4}/g) ?? []).join(' ')
}

/** C3 call 7: `contact_first:<contact>:<export>` (both `[a-z0-9_-]`) → its parts; `null` for any other id. */
export function contactFirstOf(id: string): { contact: string; exp: string } | null {
  const m = /^contact_first:([a-z0-9_-]+):([a-z0-9_-]+)$/.exec(id)
  return m ? { contact: m[1] as string, exp: m[2] as string } : null
}
