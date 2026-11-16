// 197 SHAKE-BY-CATEGORY: the picker's model, pure so it is tested without a DOM. Targets, closure and subsumption are
// 48 §3.2's; categories, groups, token sums, `/shake` words and the review lines are 197 §7–§8.
import type { ContextItem, ContextKind, ContextTarget } from '@/lib/context-197'

/** `/shake <word>` (197 §7): literal categories, not a selector language. */
export const SHAKE_WORDS = ['images', 'documents', 'surfaces', 'tools', 'notes', 'agents', 'messages', 'summary'] as const
export type ShakeWord = (typeof SHAKE_WORDS)[number]
export const SHAKE_USAGE = `usage: /shake [${SHAKE_WORDS.join('|')}]`

const WORD_KIND: Record<Exclude<ShakeWord, 'messages'>, ContextKind> = {
  images: 'image',
  documents: 'document',
  surfaces: 'surface',
  tools: 'tool',
  notes: 'note',
  agents: 'peer',
  summary: 'summary',
}

/** Stable identity of a target; `part`/`image`/`index` address original stored arrays (48 §3.1, 197 §3). */
export function key(t: ContextTarget): string {
  switch (t.kind) {
    case 'message':
      return `m${t.seq}`
    case 'head':
      return `h${t.epoch}`
    case 'binary':
      return `${key(t.owner)}/${t.part}${t.image === null ? '' : `.${t.image}`}`
    case 'fence':
      return `${key(t.owner)}#f${t.part}.${t.index}`
    case 'source':
      return `${key(t.owner)}#s${t.part}`
  }
}

/**
 * The exact set `remove_context` receives: selected targets plus their `remove_with` closure (iterated until stable),
 * deduped; a whole message or head subsumes its own binaries, fences and sources (48 §3.2, 197 §3).
 */
export function expand(items: ContextItem[], selected: Set<string>): { targets: ContextTarget[]; companions: ContextItem[] } {
  const byKey = new Map(items.map((i) => [key(i.target), i]))
  const set = new Map<string, ContextTarget>()
  const companions: ContextItem[] = []
  const queue = [...selected]
  for (let k = queue.shift(); k !== undefined; k = queue.shift()) {
    const item = byKey.get(k)
    if (!item || set.has(k)) continue
    set.set(k, item.target)
    if (!selected.has(k)) companions.push(item)
    for (const t of item.remove_with) queue.push(key(t))
  }
  // A part whose owner row is in the set goes with its row.
  const targets = [...set.values()].filter((t) => !((t.kind === 'binary' || t.kind === 'fence' || t.kind === 'source') && set.has(key(t.owner))))
  return { targets, companions }
}

export type Section = 'output' | 'conversation'

/** 197 §8: Attachments & output in this order, then Conversation. `color` is a theme token. */
export const CATEGORIES: readonly { kind: ContextKind; section: Section; name: string; color: string }[] = [
  { kind: 'image', section: 'output', name: 'Images', color: 'var(--chart-1)' },
  { kind: 'surface', section: 'output', name: 'Surfaces & diagrams', color: 'var(--chart-2)' },
  { kind: 'tool', section: 'output', name: 'Tool output', color: 'var(--chart-3)' },
  { kind: 'note', section: 'output', name: 'Harness notes', color: 'var(--chart-4)' },
  { kind: 'peer', section: 'output', name: 'Agent messages', color: 'var(--chart-5)' },
  { kind: 'document', section: 'output', name: 'Documents', color: 'var(--chart-6)' },
  { kind: 'user', section: 'conversation', name: 'Your messages', color: 'var(--fg-primary)' },
  { kind: 'assistant', section: 'conversation', name: 'My replies', color: 'var(--fg-primary)' },
  { kind: 'summary', section: 'conversation', name: 'Summary', color: 'var(--fg-primary)' },
]

/** §1 group keys in display order with their names; a key the engine adds later sorts after these under its own name. */
const GROUPS: Partial<Record<ContextKind, [string, string][]>> = {
  image: [['screenshot', 'Screenshots'], ['attachment', 'Attachments'], ['tool', 'From tools'], ['summary', 'Summary images']],
  surface: [['diagram', 'Diagrams in replies'], ['page', 'Pages'], ['app', 'Apps'], ['file', 'Files shown']],
  note: [['knowledge', 'Knowledge hints'], ['todo', 'Todo snapshots'], ['time', 'Time stamps'], ['notice', 'Child, job and reminder notices']],
}

/** Tool output shows its six largest tools by tokens; the rest fold into one group. */
export const TOOL_GROUPS = 6
export const OTHER_TOOLS = 'Other tools'

export type Group = { id: string; name: string; items: ContextItem[]; keys: string[]; tokens: number }
export type Category = (typeof CATEGORIES)[number] & { groups: Group[]; keys: string[]; tokens: number; count: number }

const sum = (items: ContextItem[]) => items.reduce((s, i) => s + i.tokens, 0)
const group = (kind: ContextKind, id: string, name: string, items: ContextItem[]): Group => ({
  id: `${kind}/${id}`,
  name,
  items,
  keys: items.map((i) => key(i.target)),
  tokens: sum(items),
})

/** The inventory by section → category → group, categories without items left out; items keep snapshot order. */
export function catalog(items: ContextItem[]): Category[] {
  const byKind = new Map<ContextKind, Map<string, ContextItem[]>>()
  for (const i of items) {
    const groups = byKind.get(i.kind) ?? new Map<string, ContextItem[]>()
    byKind.set(i.kind, groups)
    const list = groups.get(i.group)
    if (list) list.push(i)
    else groups.set(i.group, [i])
  }
  const out: Category[] = []
  for (const c of CATEGORIES) {
    const found = byKind.get(c.kind)
    if (!found) continue
    let groups: Group[]
    if (c.kind === 'tool') {
      const ranked = [...found].map(([id, list]) => group(c.kind, id, id, list)).sort((a, b) => b.tokens - a.tokens)
      groups = ranked.slice(0, TOOL_GROUPS)
      const rest = new Set(ranked.slice(TOOL_GROUPS).flatMap((g) => g.items))
      if (rest.size > 0) groups.push(group(c.kind, '', OTHER_TOOLS, items.filter((i) => rest.has(i))))
    } else {
      const named = GROUPS[c.kind] ?? []
      const rank = (id: string) => {
        const n = named.findIndex(([k]) => k === id)
        return n < 0 ? named.length : n
      }
      groups = [...found]
        .sort(([a], [b]) => rank(a) - rank(b) || a.localeCompare(b))
        .map(([id, list]) => group(c.kind, id, named.find(([k]) => k === id)?.[1] ?? (named.length ? id : c.name), list))
    }
    const keys = groups.flatMap((g) => g.keys)
    out.push({ ...c, groups, keys, tokens: sum(groups.flatMap((g) => g.items)), count: keys.length })
  }
  return out
}

export type Tri = 'none' | 'some' | 'all'

export function tri(keys: string[], selected: Set<string>): Tri {
  const n = keys.reduce((s, k) => s + (selected.has(k) ? 1 : 0), 0)
  return n === 0 ? 'none' : n === keys.length ? 'all' : 'some'
}

/** A tri-state click: all selected clears them, anything less selects them all. */
export function toggle(keys: string[], selected: Set<string>): Set<string> {
  const next = new Set(selected)
  if (tri(keys, selected) === 'all') for (const k of keys) next.delete(k)
  else for (const k of keys) next.add(k)
  return next
}

/** 197 §2: the tokens the closed target set removes; a part whose owner row is also in the set counts once, with its row. */
export function tokens(items: ContextItem[], selected: Set<string>): number {
  const byKey = new Map(items.map((i) => [key(i.target), i]))
  return expand(items, selected).targets.reduce((s, t) => s + (byKey.get(key(t))?.tokens ?? 0), 0)
}

/** `/shake <word>` preselection (197 §7): a whole category; nothing for `messages`; `summary` is the head. */
export function preselect(word: ShakeWord, items: ContextItem[]): Set<string> {
  if (word === 'messages') return new Set()
  const kind = WORD_KIND[word]
  return new Set(items.filter((i) => i.kind === kind).map((i) => key(i.target)))
}

/** The category a word opens. */
export const wordKinds = (word: ShakeWord): ContextKind[] => (word === 'messages' ? ['user', 'assistant'] : [WORD_KIND[word]])

export type ReviewLine = { label: string; tokens: number }
export type Review = {
  lines: ReviewLine[]
  companions: { count: number; tokens: number }
  /** A summary image or the head. */
  summary: boolean
  /** Fences cut out of replies. */
  diagrams: boolean
  /** Sources swapped for a marker. */
  cards: boolean
}

/** 197 §8 review: a line per category, or per group when the category is partly selected; subsumed parts count with their row. */
export function review(cats: Category[], items: ContextItem[], selected: Set<string>): Review {
  const { targets, companions } = expand(items, selected)
  const closed = new Set(targets.map(key))
  const byKey = new Map(items.map((i) => [key(i.target), i]))
  const counted = (keys: string[]) => keys.reduce((s, k) => s + (selected.has(k) && closed.has(k) ? (byKey.get(k)?.tokens ?? 0) : 0), 0)
  const lines: ReviewLine[] = []
  for (const c of cats) {
    const n = c.keys.filter((k) => selected.has(k)).length
    if (n === 0) continue
    if (n === c.count || c.groups.length === 1) {
      lines.push({ label: `${n === c.count ? 'All' : `${n} of`} ${c.count} · ${c.name}`, tokens: counted(c.keys) })
      continue
    }
    for (const g of c.groups) {
      const gn = g.keys.filter((k) => selected.has(k)).length
      if (gn === 0) continue
      lines.push({ label: `${c.name} › ${g.name} (${gn === g.keys.length ? `all ${gn}` : `${gn} of ${g.keys.length}`})`, tokens: counted(g.keys) })
    }
  }
  return {
    lines,
    companions: { count: companions.length, tokens: companions.reduce((s, i) => s + i.tokens, 0) },
    summary: targets.some((t) => t.kind === 'head' || (t.kind === 'binary' && t.owner.kind === 'head')),
    diagrams: targets.some((t) => t.kind === 'fence'),
    cards: targets.some((t) => t.kind === 'source'),
  }
}

/** `~840`, `~1.2k`, `~48k`. */
export const approx = (x: number) => (x >= 1000 ? `~${(x / 1000).toFixed(x >= 10000 ? 0 : 1)}k` : `~${x}`)
