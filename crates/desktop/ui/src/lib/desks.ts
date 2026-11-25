import type { AgentSummary } from '@/rpc/bindings/method.roster.output'

/** One of the sidebar's desks: a roster agent, and its current desk session (`null` until opened). */
export type Desk = { id: string | null; name: string; working: boolean }

/**
 * The desks the sidebar's Desks group shows: every active roster agent (not proposed, not invalid), theuth first; `id`
 * is its current desk session (`AgentSummary.desk`, roster-owned), if open yet. One per agent: a session that once was
 * an agent's desk (`SessionSummary.desk` stays on it) or an agent gone from the roster is not one.
 */
export function deskList(agents: readonly AgentSummary[]): Desk[] {
  return agents
    .filter((a) => a.status !== 'proposed' && a.status !== 'invalid')
    .map((a) => ({ id: a.desk, name: a.file.name, working: a.status === 'working' }))
    .sort((x, y) => Number(y.name === 'theuth') - Number(x.name === 'theuth'))
}

/** Current desk session id → its agent's name, for the desks that have a session (`deskList` order). */
export function deskSessions(desks: readonly Desk[]): ReadonlyMap<string, string> {
  return new Map(desks.flatMap((d) => (d.id ? [[d.id, d.name] as const] : [])))
}
