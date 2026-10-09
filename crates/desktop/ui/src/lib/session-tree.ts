import type { ObservedMeta } from '@/rpc/bindings/ObservedMeta'
import type { ObservedState } from '@/rpc/bindings/ObservedState'
import type { SessionKind } from '@/rpc/bindings/SessionKind'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import type { AgentInfo, AgentState } from '@/rpc/extra'
import type { CostBucket } from '@/rpc/bindings/CostBucket'
import { costShort, usd } from './money'

/** How many subagent rows any parent shows before `Show more`. */
export const TREE_CAP = 5

/** A subagent that still wants eyes: running, waiting on a peer, or blocked on an approval. Finished rows (idle, cancelled, error) are not. */
export const live = (state: AgentState['state']): boolean => state === 'running' || state === 'awaiting' || state === 'approval'

/** 125 §8.5: the hidden advisor child is never a tree row, a strip dot or a live session — the engine hides it; this is the belt. */
export const notAdvisor = (r: { kind?: SessionKind }): boolean => r.kind !== 'advisor'

/**
 * 140 §13 Revision 2: an autoresearch study session (the schedule's session, titled with the schedule name
 * `autoresearch-<slug>`) is reached from the Study view, never listed among the chats; its wave children hang under it.
 */
export const notStudy = (r: { title: string }): boolean => !r.title.startsWith('autoresearch-')

/** 141 §3: the tag a watch-only child's row carries — which feed describes it. */
export const sourceTag = (o: Pick<ObservedMeta, 'source'>): 'omp task' | 'opencode' | 'ACP' =>
  o.source === 'omp_task' ? 'omp task' : o.source === 'rfd' ? 'ACP' : 'opencode'

/** 141 §5.2: the tree/strip state of a watch-only child from the harness's own; `idle`/`completed`/`pending`/`unknown` all rest (Q1: idle is not done). */
export const observedState = (s: ObservedState): AgentState =>
  s === 'running'
    ? { state: 'running' }
    : s === 'requires_action'
      ? { state: 'approval', tool: 'permission' }
      : s === 'failed'
        ? { state: 'error' }
        : s === 'cancelled'
          ? { state: 'cancelled' }
          : { state: 'idle' }

/** The word a watch-only row, dot or bar shows for its state (never "observed" in the UI). */
export const observedLabel: Record<ObservedState, string> = {
  pending: 'pending',
  running: 'running',
  requires_action: 'waiting on you',
  idle: 'idle',
  completed: 'finished',
  failed: 'failed',
  cancelled: 'stopped',
  unknown: 'lost contact',
}

/** What runs the child: the parent's harness name when known, else the feed's product. */
export const harnessName = (o: Pick<ObservedMeta, 'source'>, parentAgent: string | null | undefined): string =>
  parentAgent || (o.source === 'omp_task' ? 'omp' : o.source === 'rfd' ? 'the harness' : 'opencode')

/** 141 §6: the live fields a feed keeps in `observed.detail` — omp rows carry most; opencode/RFD rows carry none. */
export type ObservedDetail = {
  agent?: string
  index?: number
  lastIntent?: string
  currentTool?: string
  toolCount?: number
  tokens?: number
  contextTokens?: number
  /** omp: the child's model context limit (Q3: the ObservedBar/header meter reads `contextTokens/contextWindow`). */
  contextWindow?: number
  /** USD as the harness reported it; shown by the cost rule (`inside_parent` greyed, `counted` via the meter). */
  cost?: number
  /** Q9: 18.8.4 progress, 0–100, when present. */
  completionPercent?: number
}

/** `observed.detail` read defensively: the JSON is the harness's, so every field is checked, never trusted. */
export function detailOf(o: Pick<ObservedMeta, 'detail'> | null | undefined): ObservedDetail {
  const d: unknown = o?.detail
  if (typeof d !== 'object' || d === null || Array.isArray(d)) return {}
  const r = d as Record<string, unknown>
  const num = (k: string) => {
    const v = r[k]
    return typeof v === 'number' && Number.isFinite(v) ? v : undefined
  }
  const str = (k: string) => {
    const v = r[k]
    return typeof v === 'string' && v !== '' ? v : undefined
  }
  return {
    agent: str('agent'),
    index: num('index'),
    lastIntent: str('lastIntent'),
    currentTool: str('currentTool'),
    toolCount: num('toolCount'),
    tokens: num('tokens'),
    contextTokens: num('contextTokens'),
    contextWindow: num('contextWindow'),
    cost: num('cost'),
    completionPercent: num('completionPercent'),
  }
}

/** What every subagent list sorts by: live rows first, then most recent activity, then newest created. */
export type Rank = { live: boolean; at: number; created: number }

export const byRank = (a: Rank, b: Rank): number => Number(b.live) - Number(a.live) || b.at - a.at || b.created - a.created

const stamp = (iso: string | null | undefined): number => (iso ? Date.parse(iso) : Number.NaN)
const latest = (...isos: (string | null | undefined)[]): number => Math.max(...isos.map(stamp).filter((n) => !Number.isNaN(n)), 0)

/**
 * A subtree row's rank. Activity is the newest of `activity.last_active` (62 §3.2: the last input, stop or turn
 * start — `updated_at` when cold), the session list's `updated_at`, and the row's live `started_at`/`finished_at`
 * (patched from events before either snapshot is refetched).
 */
export function rank(row: AgentInfo, sessions: readonly SessionSummary[]): Rank {
  const s = sessions.find((x) => x.id === row.id)
  return {
    live: live(row.state.state),
    at: latest(row.activity?.last_active, s?.updated_at, row.started_at, row.finished_at),
    created: latest(s?.created_at),
  }
}

/** The same rule over session list rows (no subtree row at hand): `running` is the live bit. */
export const rankSummary = (s: SessionSummary): Rank => ({ live: s.running, at: latest(s.updated_at), created: latest(s.created_at) })

/**
 * One parent's children, ranked: live first, most recently active next, newest created last. `expanded` shows them
 * all, else the first `limit` plus any of `keep` (the open chat and its ancestors) that would otherwise be hidden,
 * appended in rank order. `hidden` is what `Show more` would reveal.
 */
export function cappedTree(
  rows: readonly AgentInfo[],
  sessions: readonly SessionSummary[],
  expanded: boolean,
  keep: ReadonlySet<string> = new Set(),
  limit = TREE_CAP,
): { shown: AgentInfo[]; hidden: number } {
  const sorted = rows
    .map((r) => [r, rank(r, sessions)] as const)
    .sort((a, b) => byRank(a[1], b[1]))
    .map(([r]) => r)
  const shown = expanded ? sorted : [...sorted.slice(0, limit), ...sorted.slice(limit).filter((r) => keep.has(r.id))]
  return { shown, hidden: sorted.length - shown.length }
}

/** Subtree rows keyed by their parent, so each parent's list is ranked and capped on its own. */
export function childrenOf(rows: readonly AgentInfo[]): Map<string, AgentInfo[]> {
  const out = new Map<string, AgentInfo[]>()
  for (const r of rows.filter(notAdvisor)) {
    const list = out.get(r.parent)
    if (list) list.push(r)
    else out.set(r.parent, [r])
  }
  return out
}

/** How many dots the chat's subagent strip shows before a `+N` tail (93). */
export const STRIP_CAP = 24

const spawn = (r: AgentInfo): number => (r.started_at ? Date.parse(r.started_at) : Number.POSITIVE_INFINITY)
/** Spawn order: `started_at` ascending, rows without one last, name breaks ties. */
const bySpawn = (a: AgentInfo, b: AgentInfo): number => spawn(a) - spawn(b) || a.name.localeCompare(b.name)

/** The subagent strip's rows (93): the live rows of a subtree in spawn order, the first `limit` shown, the rest counted. */
export function stripRows(children: Readonly<Record<string, AgentInfo>>, limit = STRIP_CAP): { shown: AgentInfo[]; more: number } {
  const rows = Object.values(children)
    .filter((r) => notAdvisor(r) && live(r.state.state))
    .sort(bySpawn)
  return { shown: rows.slice(0, limit), more: Math.max(0, rows.length - limit) }
}

/** Those of `ids` that are rows of the flat subtree `rows` (keyed by id), plus every ancestor of theirs in it. */
export function withAncestors(rows: Readonly<Record<string, AgentInfo>>, ids: Iterable<string>): Set<string> {
  const out = new Set<string>()
  for (const id of ids) for (let cur = rows[id]; cur && !out.has(cur.id); cur = rows[cur.parent]) out.add(cur.id)
  return out
}

/** The top-level ancestor of `id` (itself when top-level or unknown). */
export function rootOf(sessions: readonly SessionSummary[], id: string | undefined): string | undefined {
  let cur = id
  for (;;) {
    const parent = sessions.find((s) => s.id === cur)?.parent
    if (!parent) return cur
    cur = parent
  }
}

/**
 * Every subagent under its top-level ancestor (all depths), keyed by the root's id — a chat row or a desk. Feeds the
 * collapsed row's running-subagent glyph and tells the Desks group which desk owns the active subagent.
 */
export function subtrees(sessions: readonly SessionSummary[]): Map<string, SessionSummary[]> {
  const out = new Map<string, SessionSummary[]>()
  for (const s of sessions) {
    if (s.parent === null || !notAdvisor(s)) continue
    const root = rootOf(sessions, s.id)
    if (root === undefined) continue
    const list = out.get(root)
    if (list) list.push(s)
    else out.set(root, [s])
  }
  return out
}

/**
 * 141 §7.6 / R3 T8: the one money cell every surface shows for a watch-only child (tree row, roster row, header
 * pill): `counted` → the child's own metered total (`session_cost(child)`), else the harness's figure; `inside_parent`
 * → the harness's figure greyed; `display_only`/`unreported` → `—`. `metered` is the child's inclusive bucket.
 */
export function watchCost(o: Pick<ObservedMeta, 'cost_rule' | 'detail'> | null | undefined, metered: CostBucket | undefined): { text: string; grey: boolean; title: string } {
  const rule = o?.cost_rule
  const reported = detailOf(o).cost
  const money = metered && (metered.returned.length > 0 || metered.estimated_for_unavailable.length > 0) ? costShort(metered) : undefined
  if (rule === 'counted') {
    return { text: money ?? (reported !== undefined ? usd(reported) : '—'), grey: false, title: 'counted — not in the harness’s own total' }
  }
  if (rule === 'inside_parent' && reported !== undefined) return { text: usd(reported), grey: true, title: 'inside parent — already in the harness’s own total' }
  return { text: '—', grey: true, title: rule === 'display_only' ? 'reported by the harness, not counted' : 'not reported by the harness' }
}
