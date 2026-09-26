import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import type { AgentInfo } from '@/rpc/extra'

/** How many subagent rows a desk shows before `Show more`. */
export const TREE_CAP = 5

/**
 * The rows of a capped tree: running first, then newest by the session list's `updated_at` (a row the list has not
 * caught up with yet falls back to its own `started_at`); `expanded` shows them all, else the first `limit`. `hidden`
 * is what `Show more` would reveal.
 */
export function cappedTree(
  rows: readonly AgentInfo[],
  sessions: readonly SessionSummary[],
  expanded: boolean,
  limit = TREE_CAP,
): { shown: AgentInfo[]; hidden: number } {
  // One sortable key per row: `1`/`0` for running, then the RFC 3339 stamp — both compare lexically, newest last.
  const sorted = rows
    .map((r) => [r, `${r.state.state === 'running' ? 1 : 0}${sessions.find((s) => s.id === r.id)?.updated_at ?? r.started_at ?? ''}`] as const)
    .sort((a, b) => b[1].localeCompare(a[1]))
    .map(([r]) => r)
  const shown = expanded ? sorted : sorted.slice(0, limit)
  return { shown, hidden: sorted.length - shown.length }
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
