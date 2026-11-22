// 101: pure helpers of the decision & review surface — the code-ref grammar of §3, the D11 "next item" rule, and the
// in-app route a pointer opens. 207: the flat list — search, sorts, filters, approval stacks, selection over entries,
// the card's reason line and countdowns, bulk targets and outcomes, and the `AttentionChanged` patch. No React, no fetches.

import type { AttentionItem, AttentionKind, AttentionOutcome, AttentionResolution } from '@/rpc/bindings/method.roster.output'
import type { Pointer } from '@/rpc/bindings/method.roster.output'
import type { AttentionTarget } from '@/rpc/bindings/method.roster.input'
import type { AttentionChanged } from '@/rpc/bindings/event.roster_2eattention.output'
import type { HistoryEntry } from '@/rpc/bindings/method.agent.output'
import { itemLabel } from './labels'

export type SortKey = 'priority' | 'expiring' | 'oldest' | 'newest' | 'agent' | 'project'

/** Menu order and labels. */
export const SORTS: readonly { key: SortKey; label: string }[] = [
  { key: 'priority', label: 'Priority' },
  { key: 'expiring', label: 'Expiring soonest' },
  { key: 'oldest', label: 'Oldest waiting' },
  { key: 'newest', label: 'Newest' },
  { key: 'agent', label: 'By agent' },
  { key: 'project', label: 'By project' },
]

export type ProjectFilter = { type: 'all' } | { type: 'none' } | { type: 'project'; name: string }
/** 208 §7.4: the list restricted to one graph subtree; `ids` = the subtree's session ids as the graph knows them now. */
export type SubtreeFilter = { session: string; label: string; ids: ReadonlySet<string> }
export type ListFilter = { kinds: ReadonlySet<string>; project: ProjectFilter; query: string; subtree: SubtreeFilter | null }
/** Every kind, every project, no query, no subtree. */
export const EMPTY_FILTER: ListFilter = { kinds: new Set(), project: { type: 'all' }, query: '', subtree: null }

/** Chip order: stop-asks, mesh, yours, receipts; kinds outside it sort after, by name. */
export const KIND_ORDER: readonly AttentionKind[] = ['approval', 'ask', 'decision', 'secret', 'handoff', 'resume', 'contact_first', 'node', 'home', 'review', 'hire', 'blocked', 'budget', 'show', 'finished', 'double']

// An engine before 207 (or a fixture built through `receiveComponent` without the new fields) leaves them undefined:
// every reader below goes through these, so such an item still lists, searches and sorts.
const pathOf = (item: AttentionItem): readonly string[] => item.path ?? []
const projectOf = (item: AttentionItem): string | null => item.project ?? null

/** Lowercased haystack: title, detail, agent, tool, task, project, path (joined by space), summary, itemLabel, session's first 8 chars. */
export function searchText(item: AttentionItem): string {
  return [item.title, item.detail, item.agent, item.tool, item.task, projectOf(item), pathOf(item).join(' '), item.summary, itemLabel(item), item.session?.slice(0, 8)]
    .filter((x): x is string => typeof x === 'string' && x !== '')
    .join('\n')
    .toLowerCase()
}

/** Every whitespace-separated term of `query` (lowercased) occurs in `searchText`; '' matches all. */
export function matchesQuery(item: AttentionItem, query: string): boolean {
  const terms = query.toLowerCase().split(/\s+/).filter(Boolean)
  if (terms.length === 0) return true
  const hay = searchText(item)
  return terms.every((t) => hay.includes(t))
}

/** Kind (empty set = any), project (`none` = project null), graph subtree (208) and query. */
export function matchesFilter(item: AttentionItem, f: ListFilter): boolean {
  if (f.kinds.size > 0 && !f.kinds.has(item.kind)) return false
  const project = projectOf(item)
  if (f.project.type === 'none' && project !== null) return false
  if (f.project.type === 'project' && project !== f.project.name) return false
  if (f.subtree && !(typeof item.session === 'string' && f.subtree.ids.has(item.session))) return false
  return matchesQuery(item, f.query)
}

const expiryOf = (i: AttentionItem): number | null => {
  const xs = [i.deadline_ms, i.expires_ms].filter((x): x is number => typeof x === 'number')
  return xs.length > 0 ? Math.min(...xs) : null
}
const agentOf = (i: AttentionItem): string => i.agent ?? pathOf(i).at(-1) ?? ''

/** Ascending by a key where `null` sorts last. */
const nullsLast = <T>(a: T | null, b: T | null, cmp: (x: T, y: T) => number): number => (a === null ? (b === null ? 0 : 1) : b === null ? -1 : cmp(a, b))

const COMPARE: Record<Exclude<SortKey, 'priority'>, (a: AttentionItem, b: AttentionItem) => number> = {
  expiring: (a, b) => nullsLast(expiryOf(a), expiryOf(b), (x, y) => x - y),
  oldest: (a, b) => (a.waiting_since_ms ?? a.at_ms) - (b.waiting_since_ms ?? b.at_ms),
  newest: (a, b) => b.at_ms - a.at_ms,
  agent: (a, b) => nullsLast(agentOf(a) || null, agentOf(b) || null, (x, y) => x.localeCompare(y)),
  project: (a, b) => nullsLast(projectOf(a), projectOf(b), (x, y) => x.localeCompare(y)),
}

/**
 * Stable; ties keep the input (engine) order. priority: input order · expiring: min(deadline_ms, expires_ms) asc, items
 * with neither last · oldest: (waiting_since_ms ?? at_ms) asc · newest: at_ms desc · agent: (agent ?? path.at(-1) ?? '')
 * localeCompare, '' last · project: project localeCompare, null last.
 */
export function sortItems(items: readonly AttentionItem[], key: SortKey): AttentionItem[] {
  if (key === 'priority') return [...items]
  const cmp = COMPARE[key]
  return items
    .map((item, i) => ({ item, i }))
    .sort((a, b) => cmp(a.item, b.item) || a.i - b.i)
    .map((x) => x.item)
}

export type Entry =
  | { type: 'item'; key: string; item: AttentionItem }
  | { type: 'stack'; key: string; items: AttentionItem[] }

/** `stack:<session>:<tool>` for an approval with both, else null. */
export function stackKey(item: AttentionItem): string | null {
  return item.kind === 'approval' && item.session && item.tool ? `stack:${item.session}:${item.tool}` : null
}

/** Approvals sharing a stackKey collapse into one entry at the first member's position; a lone member stays an item. */
export function stackEntries(items: readonly AttentionItem[]): Entry[] {
  const members = new Map<string, AttentionItem[]>()
  for (const item of items) {
    const k = stackKey(item)
    if (k !== null) members.set(k, [...(members.get(k) ?? []), item])
  }
  const out: Entry[] = []
  const placed = new Set<string>()
  for (const item of items) {
    const k = stackKey(item)
    const group = k === null ? undefined : members.get(k)
    if (k === null || !group || group.length < 2) {
      out.push({ type: 'item', key: item.id, item })
    } else if (!placed.has(k)) {
      placed.add(k)
      out.push({ type: 'stack', key: k, items: group })
    }
  }
  return out
}

/** dropExpired(now) → matchesFilter → sortItems → stackEntries. */
export function visibleEntries(items: readonly AttentionItem[], f: ListFilter, sort: SortKey, now: number): Entry[] {
  return stackEntries(sortItems(items.filter((i) => !isExpired(i, now) && matchesFilter(i, f)), sort))
}

export const entryIds = (e: Entry): string[] => (e.type === 'item' ? [e.item.id] : e.items.map((i) => i.id))
export const entryItems = (e: Entry): AttentionItem[] => (e.type === 'item' ? [e.item] : e.items)

/** The item a card shows: the item, or a stack's first member. */
export function entryHead(e: Entry): AttentionItem {
  return e.type === 'item' ? e.item : (e.items[0] as AttentionItem)
}

/**
 * 207 review 12: the selection holds ITEM ids, snapshotted when the user selected — a stack that gains a member after
 * that is only partly selected, and a single approval that becomes a stack keeps its selection. Ids in entry order.
 */
export function selectedIds(entries: readonly Entry[], selection: ReadonlySet<string>): string[] {
  return entries.flatMap(entryIds).filter((id) => selection.has(id))
}

/** True when the selection is non-empty and every selected item is an approval. */
export function allApprovals(entries: readonly Entry[], selection: ReadonlySet<string>): boolean {
  const items = entries.flatMap(entryItems).filter((i) => selection.has(i.id))
  return items.length > 0 && items.every((i) => i.kind === 'approval')
}

/** How much of an entry is selected: a stack whose newer members the user never saw is `some`. */
export function entrySelection(e: Entry, selection: ReadonlySet<string>): 'all' | 'some' | 'none' {
  const ids = entryIds(e)
  const n = ids.filter((id) => selection.has(id)).length
  return n === 0 ? 'none' : n === ids.length ? 'all' : 'some'
}

/**
 * Toggle (an entry fully selected loses all its ids, otherwise gains the members it shows now) or range-select (every
 * entry from `anchor` to `key`, both included, gains its members; an absent anchor acts as a toggle). A new set.
 */
export function selectEntries(selection: ReadonlySet<string>, entries: readonly Entry[], anchor: string | null, key: string, mode: 'toggle' | 'range'): Set<string> {
  const next = new Set(selection)
  const b = entries.findIndex((e) => e.key === key)
  const a = anchor === null ? -1 : entries.findIndex((e) => e.key === anchor)
  if (b < 0) return next
  if (mode === 'range' && a >= 0) {
    for (const e of entries.slice(Math.min(a, b), Math.max(a, b) + 1)) for (const id of entryIds(e)) next.add(id)
    return next
  }
  const e = entries[b] as Entry
  const all = entrySelection(e, selection) === 'all'
  for (const id of entryIds(e)) (all ? next.delete(id) : next.add(id))
  return next
}

const RISK_RANK: Record<string, number> = { destructive: 2, mutating: 1, read_only: 0 }

/**
 * 207 review 3: whether a keyboard action (A, D, S, E) on the focused entry only opens it — a collapsed stack shows its
 * members first and the next press acts; a single card, an open stack, or a selection the user built acts at once.
 */
export function opensFirst(entry: Entry, expanded: ReadonlySet<string>, selecting: boolean): boolean {
  return !selecting && entry.type === 'stack' && !expanded.has(entry.key)
}

/** A stack's card shows its highest risk, whichever member carries it. */
export function stackRisk(items: readonly AttentionItem[]): AttentionItem['risk'] {
  let best: AttentionItem['risk'] = null
  for (const i of items) if (i.risk && (best === null || (RISK_RANK[i.risk] ?? 0) > (RISK_RANK[best] ?? 0))) best = i.risk
  return best
}

/**
 * 207 review 2 and 13: where focus (or the detail pane) goes when `key` left the list — itself while still present, else
 * the first entry after it in the previous order that survived, else the nearest one before it, else the first entry.
 */
export function focusAfter(prev: readonly string[], next: readonly string[], key: string | null): string | null {
  if (key !== null && next.includes(key)) return key
  const i = key === null ? -1 : prev.indexOf(key)
  if (i < 0) return next[0] ?? null
  const alive = new Set(next)
  for (let j = i + 1; j < prev.length; j++) if (alive.has(prev[j] as string)) return prev[j] as string
  for (let j = i - 1; j >= 0; j--) if (alive.has(prev[j] as string)) return prev[j] as string
  return null
}

/**
 * 207 review 13: the item the detail pane opens once `id` leaves the displayed list — another member of its stack, else
 * the head of the entry `focusAfter` picks; null when nothing else is listed.
 */
export function nextItemAfter(entries: readonly Entry[], id: string): string | null {
  const mine = entries.find((e) => entryIds(e).includes(id))
  if (!mine) return null
  const sibling = entryIds(mine).find((x) => x !== id)
  if (sibling) return sibling
  const keys = entries.map((e) => e.key)
  const key = focusAfter(keys, keys.filter((k) => k !== mine.key), mine.key)
  const e = entries.find((x) => x.key === key)
  return e ? entryHead(e).id : null
}

/**
 * 207 review 11: a persisted kind or project filter whose value is gone leaves "Nothing matches" with no chip to undo:
 * keep only kinds present in `kinds` and a project present in `projects` (`none` needs a project-less item). The same
 * object when nothing was pruned.
 */
export function pruneFilter(f: ListFilter, kinds: readonly string[], projects: readonly (string | null)[]): ListFilter {
  const kept = [...f.kinds].filter((k) => kinds.includes(k))
  const project = f.project.type === 'none' ? projects.includes(null) : f.project.type === 'project' ? projects.includes(f.project.name) : true
  if (kept.length === f.kinds.size && project) return f
  return { ...f, kinds: new Set(kept), project: project ? f.project : { type: 'all' } }
}

/**
 * 207 review 14: ms until the list must re-render — every second while a visible item counts down, else at the nearest
 * expiry (a stopped card leaves on time), at most 30 s.
 */
export function nextTick(items: readonly AttentionItem[], now: number): number {
  let wait = 30_000
  for (const i of items) {
    if (typeof i.deadline_ms === 'number' && i.deadline_ms > now - 1000) return 1000
    if (typeof i.expires_ms === 'number' && i.expires_ms > now) wait = Math.min(wait, i.expires_ms - now + 50)
  }
  return Math.max(250, wait)
}

/** Kinds present with counts, in KIND_ORDER then unknown kinds by name. */
export function kindCounts(items: readonly AttentionItem[]): { kind: string; n: number }[] {
  const n = new Map<string, number>()
  for (const i of items) n.set(i.kind, (n.get(i.kind) ?? 0) + 1)
  const rank = (k: string) => {
    const r = (KIND_ORDER as readonly string[]).indexOf(k)
    return r < 0 ? KIND_ORDER.length : r
  }
  return [...n.entries()].sort(([a], [b]) => rank(a) - rank(b) || a.localeCompare(b)).map(([kind, count]) => ({ kind, n: count }))
}

/** Projects present with counts, by name; null ("No project") last when present. */
export function projectsOf(items: readonly AttentionItem[]): { name: string | null; n: number }[] {
  const n = new Map<string | null, number>()
  for (const i of items) n.set(projectOf(i), (n.get(projectOf(i)) ?? 0) + 1)
  return [...n.entries()].sort(([a], [b]) => nullsLast(a, b, (x, y) => x.localeCompare(y))).map(([name, count]) => ({ name, n: count }))
}

/** '1:42', '12:05', '1:02:03'; ≤ 0 → '0:00'. */
export function countdown(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000))
  const h = Math.floor(s / 3600)
  const mm = String(Math.floor((s % 3600) / 60))
  const ss = String(s % 60).padStart(2, '0')
  return h > 0 ? `${h}:${mm.padStart(2, '0')}:${ss}` : `${mm}:${ss}`
}

/** null without deadline_ms; 'due now' when passed; else 'denies itself in …' (approval) · 'double answers in …' (double) · 'times out in …'. */
export function deadlineText(item: AttentionItem, now: number): string | null {
  const d = item.deadline_ms
  if (typeof d !== 'number') return null
  if (now >= d) return 'due now'
  const verb = item.kind === 'approval' ? 'denies itself in' : item.kind === 'double' ? 'double answers in' : 'times out in'
  return `${verb} ${countdown(d - now)}`
}

/** 207 §3.3 step 7, recomputed at `now`: 'waiting 12 min' / 'waiting 2 h', 'stopped 5 min ago', '50 min ago'; null under a minute. */
function ageText(item: AttentionItem, now: number): string | null {
  const since = item.waiting_since_ms ?? item.at_ms
  const mins = Math.floor(Math.max(0, now - since) / 60_000)
  if (mins < 1) return null
  const span = mins >= 60 ? `${Math.floor(mins / 60)} h` : `${mins} min`
  if (item.waiting_since_ms !== null && item.waiting_since_ms !== undefined) return `waiting ${span}`
  return item.kind === 'finished' ? `stopped ${span} ago` : `${span} ago`
}

const reasonText = (item: AttentionItem, signal: 'live' | 'task'): string | null => (item.reasons ?? []).find((r) => r.signal === signal)?.text ?? null

/**
 * The card's reason line: the first two present of, in this order, blocks · deadline (text recomputed by deadlineText) ·
 * risk · live · task (only when priority is urgent or high) · age; joined ' · '; '' when none. Base never shows.
 * Time-dependent parts (deadline, age) are computed here from the item's timestamps, never read from the snapshot.
 */
export function reasonLine(item: AttentionItem, now: number): string {
  const blocks = item.blocks ?? 0
  const parts = [
    blocks > 0 ? `blocks ${blocks} ${blocks === 1 ? 'agent' : 'agents'}` : null,
    deadlineText(item, now),
    item.risk === 'destructive' || item.risk === 'mutating' ? item.risk : null,
    reasonText(item, 'live'),
    item.priority === 'urgent' || item.priority === 'high' ? (reasonText(item, 'task') ?? `${item.priority} priority`) : null,
    ageText(item, now),
  ]
  return parts
    .filter((p): p is string => p !== null)
    .slice(0, 2)
    .join(' · ')
}

/** 'release-2.5 · Ship 2.5 release › builder' — project (when set) then path joined ' › '; '' when both empty. */
export function pathText(item: AttentionItem): string {
  return [projectOf(item), pathOf(item).join(' › ')].filter(Boolean).join(' · ')
}

/** expires_ms set and reached. */
export function isExpired(item: AttentionItem, now: number): boolean {
  return typeof item.expires_ms === 'number' && item.expires_ms <= now
}

/** The key after/before `current` (delta 1/-1), clamped; the first key when current is null or absent; null for []. */
export function moveFocus(keys: readonly string[], current: string | null, delta: 1 | -1): string | null {
  if (keys.length === 0) return null
  const i = current === null ? -1 : keys.indexOf(current)
  if (i < 0) return keys[0] ?? null
  return keys[Math.min(keys.length - 1, Math.max(0, i + delta))] ?? null
}

/** { ids } for a bulk call. */
export function bulkTarget(ids: readonly string[]): AttentionTarget {
  return { ids: [...ids] }
}

const OUTCOME_WORDS: readonly [AttentionResolution, string][] = [
  ['hidden', 'dismissed'],
  ['snoozed', 'snoozed'],
  ['left_to_agent', 'left to agents'],
  ['left_for_later', 'left for later'],
  ['not_run', 'not run'],
  ['proceed_without', 'told to go without'],
  ['gone', 'already gone'],
  ['failed', 'failed'],
]

/** '12 dismissed · 3 left to agents · 2 not run · 1 already gone' — one part per resolution present, in AttentionResolution order. */
export function outcomeSummary(outcomes: readonly AttentionOutcome[]): string {
  return OUTCOME_WORDS.map(([r, words]) => [outcomes.filter((o) => o.resolution === r).length, words] as const)
    .filter(([n]) => n > 0)
    .map(([n, words]) => `${n} ${words}`)
    .join(' · ')
}

export type Cursor = { epoch: number; seq: number }

/**
 * Apply one AttentionChanged to the cached list.
 * seq 0 (engine before 207) or no cache → { next: cache, refetch: true, cursor: seq ? {epoch_ms, seq} : cursor }.
 * An event this cursor already passed (same epoch, seq <= cursor.seq) is stale: { next: undefined, refetch: false, cursor }.
 * Else: drop ids_removed; upsert ev.items keeping the cached item's alert (alert of a new id: null); next = ev.order mapped
 * through the map (ids not in order are dropped). refetch when: a gap (cursor set and (cursor.epoch !== ev.epoch_ms or
 * ev.seq !== cursor.seq + 1)), an id of order missing from the map, next.length !== ev.count, or ev.alerts_changed.
 * cursor = { epoch: ev.epoch_ms, seq: ev.seq }.
 */
export function applyChange(cache: readonly AttentionItem[] | undefined, ev: AttentionChanged, cursor: Cursor | null): { next: AttentionItem[] | undefined; refetch: boolean; cursor: Cursor | null } {
  // `?? 0` / `?? []`: an engine before 207 sends none of the new fields at all.
  const seq = ev.seq ?? 0
  if (seq === 0 || !cache) return { next: cache ? [...cache] : undefined, refetch: true, cursor: seq ? { epoch: ev.epoch_ms, seq } : cursor }
  if (cursor !== null && cursor.epoch === ev.epoch_ms && seq <= cursor.seq) return { next: undefined, refetch: false, cursor }
  const map = new Map(cache.map((i) => [i.id, i]))
  for (const id of ev.ids_removed) map.delete(id)
  for (const item of ev.items ?? []) map.set(item.id, { ...item, alert: map.get(item.id)?.alert ?? null })
  const order = ev.order ?? []
  const missing = order.some((id) => !map.has(id))
  const next = order.flatMap((id) => map.get(id) ?? [])
  const gap = cursor !== null && (cursor.epoch !== ev.epoch_ms || seq !== cursor.seq + 1)
  return { next, refetch: gap || missing || next.length !== ev.count || ev.alerts_changed === true, cursor: { epoch: ev.epoch_ms, seq } }
}

/** 207 review 9: the patcher's state — the cursor, a list refetch in flight, and whether an event landed during it. */
export type PatchState = { cursor: Cursor | null; inflight: boolean; dirty: boolean }
export const PATCH_START: PatchState = { cursor: null, inflight: false, dirty: false }

/**
 * One event: `applyChange`, then decide the refetch. Never two refetches at once: an event that needs one while one
 * runs, or that patched the cache while one runs (the older snapshot will land over it), marks the state dirty.
 */
export function patchEvent(state: PatchState, cache: readonly AttentionItem[] | undefined, ev: AttentionChanged): { state: PatchState; next: AttentionItem[] | undefined; refetch: boolean } {
  const r = applyChange(cache, ev, state.cursor)
  const stale = r.next === undefined && !r.refetch && cache !== undefined
  if (stale) return { state, next: undefined, refetch: false }
  if (state.inflight) return { state: { ...state, cursor: r.cursor, dirty: true }, next: r.next, refetch: false }
  return { state: { ...state, cursor: r.cursor, inflight: r.refetch }, next: r.next, refetch: r.refetch }
}

/** The refetch landed: refetch once more when an event arrived meanwhile, else idle. */
export function patchSettled(state: PatchState): { state: PatchState; refetch: boolean } {
  return state.dirty ? { state: { ...state, inflight: true, dirty: false }, refetch: true } : { state: { ...state, inflight: false }, refetch: false }
}

/** An item an action took out of the list, with where it stood. */
export type Removed = { item: AttentionItem; index: number }

/** 207 review 7: put optimistically removed items back where they stood (a failed action); ids already back are skipped. */
export function restoreItems(list: readonly AttentionItem[], removed: readonly Removed[]): AttentionItem[] {
  const out = [...list]
  for (const r of [...removed].sort((a, b) => a.index - b.index)) if (!out.some((i) => i.id === r.item.id)) out.splice(Math.min(r.index, out.length), 0, r.item)
  return out
}

/** 207 review 5: one approval's Allow / Deny — `done`, `gone` (already answered elsewhere) or `failed` (still pending). */
export type ResolveOutcome = { id: string; result: 'done' | 'gone' | 'failed'; error: string | null }

/** '3 allowed · 1 already answered · 1 failed'; '' for none. */
export function resolveSummary(outcomes: readonly ResolveOutcome[], decision: 'allow' | 'deny'): string {
  const n = (r: ResolveOutcome['result']) => outcomes.filter((o) => o.result === r).length
  return [
    [n('done'), decision === 'allow' ? 'allowed' : 'denied'],
    [n('gone'), 'already answered'],
    [n('failed'), 'failed, still waiting'],
  ]
    .filter(([k]) => (k as number) > 0)
    .map(([k, w]) => `${k} ${w}`)
    .join(' · ')
}

export const CONTEXT_MAX = 600

/** `text` cut to `max` chars on a word boundary when one is near, with "…"; shorter text verbatim. */
export function clampText(text: string, max = CONTEXT_MAX): string {
  const t = text.trim()
  if (t.length <= max) return t
  const cut = t.slice(0, max)
  const space = cut.lastIndexOf(' ')
  return `${(space > max * 0.6 ? cut.slice(0, space) : cut).trimEnd()}…`
}

/**
 * 104 A7: the agent's last assistant text stored before `beforeMs` — a row without a timestamp counts as before (older
 * engines stored none); `null` when the page has no such row.
 */
export function lastAssistantBefore(history: readonly HistoryEntry[], beforeMs: number): string | null {
  for (let i = history.length - 1; i >= 0; i--) {
    const h = history[i]
    if (!h || h.role !== 'assistant' || !h.text.trim()) continue
    const at = h.context?.at ? Date.parse(h.context.at) : NaN
    if (Number.isNaN(at) || at <= beforeMs) return h.text
  }
  return null
}

export type CodeRef = { path: string; start: number; end: number }

/** `src/a.rs#L10-L24` → `{ path, start: 10, end: 24 }`; `#L10` → start = end = 10; `null` when the ref is not of that shape. */
export function parseCodeRef(ref: string): CodeRef | null {
  const m = /^(.+?)#L(\d+)(?:-L(\d+))?$/.exec(ref)
  if (!m) return null
  const start = Number(m[2])
  const end = m[3] ? Number(m[3]) : start
  return start >= 1 && end >= start ? { path: m[1] as string, start, end } : null
}

export type PointerRoute =
  | { to: '/artifacts/$callId'; params: { callId: string } }
  | { to: '/notes/$id'; params: { id: string } }
  | { to: '/knowledge/$'; params: { _splat: string } }
  | { to: '/c/$session'; params: { session: string } }
  | { to: '/meetings/$id'; params: { id: string } }
  | { to: '/tasks/$id'; params: { id: string } }

/** The in-app route a pointer opens in its viewer; `null` for `url` (external), `code` and `diff` (inline evidence). */
export function pointerRoute(p: Pointer): PointerRoute | null {
  switch (p.kind) {
    case 'artifact':
      return { to: '/artifacts/$callId', params: { callId: p.ref } }
    case 'note':
      return { to: '/notes/$id', params: { id: p.ref } }
    case 'knowledge':
      return { to: '/knowledge/$', params: { _splat: p.ref } }
    case 'session':
      return { to: '/c/$session', params: { session: p.ref } }
    case 'meeting':
      return { to: '/meetings/$id', params: { id: p.ref } }
    case 'task':
      return { to: '/tasks/$id', params: { id: p.ref } }
    default:
      return null
  }
}

/** The host of a `url` pointer for its row, or the ref itself when it does not parse. */
export function urlHost(ref: string): string {
  try {
    return new URL(ref).host
  } catch {
    return ref
  }
}
