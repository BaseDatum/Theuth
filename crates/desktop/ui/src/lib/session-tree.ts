import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import type { AgentInfo, AgentState } from '@/rpc/extra'

/** How many subagent rows any parent shows before `Show more`. */
export const TREE_CAP = 5

/** A subagent that still wants eyes: running, waiting on a peer, or blocked on an approval. Finished rows (idle, cancelled, error) are not. */
export const live = (state: AgentState['state']): boolean => state === 'running' || state === 'awaiting' || state === 'approval'

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
  for (const r of rows) {
    const list = out.get(r.parent)
    if (list) list.push(r)
    else out.set(r.parent, [r])
  }
  return out
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
    if (s.parent === null) continue
    const root = rootOf(sessions, s.id)
    if (root === undefined) continue
    const list = out.get(root)
    if (list) list.push(s)
    else out.set(root, [s])
  }
  return out
}
