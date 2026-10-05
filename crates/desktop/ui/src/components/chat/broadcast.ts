import { chatName } from '@/lib/attach'
import type { BroadcastReceipt } from '@/rpc/bindings/BroadcastReceipt'
import type { SessionActivity } from '@/rpc/bindings/SessionActivity'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import type { AgentInfo } from '@/rpc/extra'

/** `list_activity` rows carry their depth from their own root: 0 is a root (62 §3.2). */
export const isRoot = (r: AgentInfo) => r.depth === 0

/**
 * What a recipient is called, from the session list (66 §8): a subagent's peer name, else `chatName`. `fallback`
 * (the picker row's roster name, a receipt row's own title) stands in while the list lacks the session; never the id.
 */
export function title(id: string, sessions: readonly SessionSummary[] | undefined, fallback = ''): string {
  const s = sessions?.find((x) => x.id === id)
  return s ? s.name || chatName(s) : fallback || 'Untitled chat'
}

/** Mid-turn by state alone: `Running | Awaiting | Approval` (62 §0.2); a pre-62 engine reports nothing. */
export const busy = (a: SessionActivity | undefined) =>
  a?.state.state === 'running' || a?.state.state === 'awaiting' || a?.state.state === 'approval'

/** What `BroadcastTargets::Running` selects: roots that are busy or own a running job/PTY (62 §0.2). */
export function runningRoots(rows: AgentInfo[]): string[] {
  return rows
    .filter((r) => isRoot(r) && (busy(r.activity) || (r.activity?.jobs ?? 0) + (r.activity?.ptys ?? 0) > 0))
    .map((r) => r.id)
}

/** What `BroadcastTargets::Roots` selects. */
export const allRoots = (rows: AgentInfo[]): string[] => rows.filter(isRoot).map((r) => r.id)

/** The roster name of a desk root (`SessionSummary.desk`, 66 §8); `undefined` for every other session. */
export const deskOf = (id: string, sessions: readonly SessionSummary[] | undefined): string | undefined =>
  sessions?.find((s) => s.id === id)?.desk || undefined

/** Stable sort key: the `theuth` desk ahead of the rest (the sidebar's Desks order). */
const theuthFirst = (sessions: readonly SessionSummary[] | undefined) => (x: string, y: string) =>
  Number(deskOf(y, sessions) === 'theuth') - Number(deskOf(x, sessions) === 'theuth')

/** What "All desks" selects: roots whose session is a desk, in `rows` order with the `theuth` desk first (BROADCAST-DESKS D1). */
export function deskRoots(rows: AgentInfo[], sessions: readonly SessionSummary[] | undefined): string[] {
  return rows
    .filter((r) => isRoot(r) && deskOf(r.id, sessions))
    .map((r) => r.id)
    .sort(theuthFirst(sessions))
}

/** The picker's two sections: groups whose root is a desk (theuth first), then the rest in input order (D2). */
export function splitGroups(groups: Group[], sessions: readonly SessionSummary[] | undefined): { desks: Group[]; rest: Group[] } {
  const by = theuthFirst(sessions)
  return {
    desks: groups.filter((g) => deskOf(g.root.id, sessions)).sort((x, y) => by(x.root.id, y.root.id)),
    rest: groups.filter((g) => !deskOf(g.root.id, sessions)),
  }
}

/** A section header's tri-state: every root of the section ticked, some, or none (an empty section is `none`). */
export function sectionState(ids: ReadonlySet<string>, rootIds: readonly string[]): 'all' | 'some' | 'none' {
  const n = rootIds.filter((id) => ids.has(id)).length
  return n === 0 ? 'none' : n === rootIds.length ? 'all' : 'some'
}

export type Group = { root: AgentInfo; children: AgentInfo[] }

/**
 * Roots in input order, each with every descendant present in `rows` (input order, own `depth` for
 * indentation). A row whose root is absent stands as its own group.
 */
export function groupRows(rows: AgentInfo[]): Group[] {
  const byId = new Map(rows.map((r) => [r.id, r]))
  const groups = new Map<string, Group>()
  for (const r of rows) if (isRoot(r)) groups.set(r.id, { root: r, children: [] })
  for (const r of rows) {
    if (isRoot(r)) continue
    let p = byId.get(r.parent)
    while (p && !isRoot(p)) p = byId.get(p.parent)
    const g = p && groups.get(p.id)
    if (g) g.children.push(r)
    else groups.set(r.id, { root: r, children: [] })
  }
  return [...groups.values()]
}

/** `2 jobs · 1 pty`; empty when nothing runs. */
export function counts(a: SessionActivity | undefined): string {
  if (!a) return ''
  const parts: string[] = []
  if (a.jobs > 0) parts.push(`${a.jobs} job${a.jobs === 1 ? '' : 's'}`)
  if (a.ptys > 0) parts.push(`${a.ptys} pty${a.ptys === 1 ? '' : 's'}`)
  return parts.join(' · ')
}

/** Dry-run footer: `4 sessions · 2 busy → queued, 2 idle → start now`, from each row's `before.state`. */
export function summarize(receipt: BroadcastReceipt): string {
  const n = receipt.rows.length
  const b = receipt.rows.filter((r) => busy(r.before)).length
  return `${n} session${n === 1 ? '' : 's'} · ${b} busy → queued, ${n - b} idle → start now`
}

/** Toast lines, one per row (`title — started | queued | skipped: reason | failed: error`), failures first. */
export function outcomeLines(receipt: BroadcastReceipt, sessions: readonly SessionSummary[] | undefined): string[] {
  const line = (r: BroadcastReceipt['rows'][number]) => {
    const o = r.outcome
    const what =
      o.outcome === 'started'
        ? 'started'
        : o.outcome === 'queued'
          ? o.delivery === 'next_turn'
            ? 'queued (next turn)'
            : 'queued'
          : o.outcome === 'skipped'
            ? `skipped: ${o.reason}`
            : `failed: ${o.error}`
    return `${title(r.session, sessions, r.title)} — ${what}`
  }
  const failed = receipt.rows.filter((r) => r.outcome.outcome === 'failed')
  const rest = receipt.rows.filter((r) => r.outcome.outcome !== 'failed')
  return [...failed, ...rest].map(line)
}
