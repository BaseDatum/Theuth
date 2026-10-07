import type { ActivityEntry } from '@/rpc/bindings/ActivityEntry'
import type { AgentSummary } from '@/rpc/bindings/AgentSummary'
import type { AttentionItem } from '@/rpc/bindings/AttentionItem'
import type { ScheduleInfo } from '@/rpc/bindings/ScheduleInfo'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import type { Task } from '@/rpc/bindings/Task'
import { HOUR_MS, rng, type SceneData, startOfDay } from './model'

/**
 * Synthetic data that reproduces the approved mockup: 6 needs-you items, 7 rings, 8 done, 3 running, ~230 field lines,
 * a week of arrivals, spend rows and a stream. Timed relative to `now` so the arc reads right at any hour.
 */

const M = 60_000
const pick = <T,>(arr: readonly T[], u: number): T => arr[Math.floor(u * arr.length)] as T
const H = HOUR_MS

const agent = (name: string, status: AgentSummary['status'], open_tasks: number, spent_month: number, cap: number | null, desk: string | null): AgentSummary => ({
  file: { name, title: name, reports_to: 'theuth', model: null, effort: null, tools: null, mcp: null, budget: cap === null ? { type: 'none' } : { type: 'monthly', amount: cap, currency: 'USD' }, capabilities: null, paused: status === 'paused', proposed_by: null, runs_in: 'any', body: '' },
  status,
  desk,
  open_tasks,
  spent_month,
})

const task = (id: string, title: string, owner: string, state: Task['state'], updated_ms: number, progress_at_ms: number | null = null): Task => ({
  id,
  title,
  brief: '',
  owner: owner === 'user' ? { type: 'user' } : { type: 'agent', name: owner },
  state,
  reviewer: null,
  blockers: [],
  blocks: [],
  parent: null,
  project: null,
  where: null,
  harness: null,
  external: [],
  thread: null,
  checkout: progress_at_ms === null ? null : { session: `s-${id}`, engine: 'mac', epoch: 1, progress_at_ms, orphaned: false, taken: false },
  schedule: null,
  created_by: 'user',
  created_ms: updated_ms - 2 * H,
  updated_ms,
  no_progress_count: 0,
  next_wake_after_ms: null,
  done_when: null,
  attachments: [],
})

const session = (id: string, title: string, updated: number, extra: Partial<SessionSummary> = {}): SessionSummary => ({
  id,
  created_at: new Date(updated - H).toISOString(),
  updated_at: new Date(updated).toISOString(),
  title,
  tags: [],
  model: 'sim',
  parent: null,
  name: null,
  effort: null,
  agent: null,
  cwd: null,
  pinned: false,
  archived: false,
  project: null,
  retention: 'durable',
  running: false,
  owner: '',
  epoch: 0,
  here: true,
  ...extra,
})

const schedule = (name: string, next: number | null, last: number | null, running: boolean, last_result: string | null): ScheduleInfo => ({
  name,
  when: 'cron 0 9 * * *',
  describe: 'daily',
  enabled: true,
  quiet: false,
  next: next === null ? null : new Date(next).toISOString(),
  last: last === null ? null : new Date(last).toISOString(),
  last_result,
  session: null,
  running,
  error: null,
  until: null,
  max_runs: null,
  runs: 0,
  approvals: true,
  cwd: null,
  agent: null,
  project: null,
  when_sensor: null,
  fire: null,
  mode: null,
  memory: null,
  expires: null,
  last_check: null,
})

const need = (id: string, kind: AttentionItem['kind'], title: string, at_ms: number, agent: string | null, task: string | null): AttentionItem => ({
  id,
  kind,
  title,
  detail: null,
  at_ms,
  task,
  agent,
  session: null,
  approval: null,
  decision: null,
})

/** uuidv7-shaped task ids whose short id (last 6) reads like the mock's. */
export const T = (n: string) => `0192a1b2-c3d4-7e5f-8a9b-00000000${n}`

export const FIXTURE_AGENTS = ['ops-desk', 'research', 'builder', 'analyst', 'docs-desk'] as const

export function sceneFixture(now: number): Required<SceneData> {
  const midnight = startOfDay(now)
  const today = (h: number) => Math.min(now - M, midnight + h * H)
  const agents = [
    agent('ops-desk', 'working', 4, 12.4, 40, 'desk-ops'),
    agent('research', 'working', 3, 9.1, 25, 'desk-research'),
    agent('builder', 'blocked', 2, 6.8, 10, 'desk-builder'),
    agent('analyst', 'idle', 1, 0, null, 'desk-analyst'),
    agent('docs-desk', 'paused', 0, 2.9, 3, null),
  ]
  const attention: AttentionItem[] = [
    need('approval:ap-4821', 'approval', 'builder · cargo publish --dry-run', now - 12 * M, 'builder', null),
    need(`decision:${T('1198')}:dns`, 'decision', 'ops-desk · rotate relay cert now or at the window?', now - 31 * M, 'ops-desk', T('1198')),
    need(`review:${T('1188')}`, 'review', 'research · design 126 draft ready', now - 1 * H, 'research', T('1188')),
    need('hire:analyst-2', 'hire', 'ops-desk proposes an analyst desk', now - 3 * H, 'ops-desk', null),
    need(`blocked:${T('1203')}`, 'blocked', 'windows build host unreachable', now - 26 * H, 'builder', T('1203')),
    need('secret:gh-token', 'secret', 'GITHUB_TOKEN rotation requested', now - 50 * H, null, null),
  ]
  const tasks: Task[] = [
    task(T('1214'), 'wire HUD scene into /', 'ops-desk', 'doing', now - 12 * M, now - 12 * M),
    task(T('1210'), 'design 126 review', 'research', 'doing', now - 40 * M, now - 40 * M),
    task(T('1213'), 'punch card arrivals', 'builder', 'doing', now - 55 * M, now - 55 * M),
    task(T('1212'), 'release notes 2.4', 'ops-desk', 'done', today(19.8)),
    task(T('1207'), 'relay cert rotation', 'ops-desk', 'done', today(18.5)),
    task(T('1201'), 'landed main', 'research', 'done', today(17.2)),
    task(T('1199'), 'artifact report.pdf', 'research', 'done', today(14.1)),
    task(T('1195'), 'sandbox image', 'builder', 'done', today(12.4)),
    task(T('1190'), 'knowledge +2', 'user', 'done', today(10.0)),
    task(T('1180'), 'mesh relay', 'analyst', 'done', today(8.3)),
    task(T('1188'), 'design 126 draft', 'research', 'review', now - 1 * H),
    task(T('1203'), 'windows build host', 'builder', 'blocked', now - 26 * H),
    task(T('1170'), 'old idea', 'user', 'cancelled', today(9.0)),
    task(T('1160'), 'backlog item', 'user', 'backlog', now - 3 * 24 * H),
  ]
  const routines = [
    schedule('morning-brief', midnight + 30 * H, today(6.0), false, 'ok'),
    schedule('standup-notes', midnight + 33 * H, today(9.0), false, 'ok'),
    schedule('lunch-digest', midnight + 36.5 * H, today(12.5), false, 'ok'),
    schedule('nightly-digest', midnight + 42 * H, now - 18 * M, true, 'ok'),
    schedule('knowledge-sync', midnight + 23.5 * H, midnight - 0.5 * H, false, 'error: offline'),
  ]
  const sessions: SessionSummary[] = [
    session('chat-1', 'HUD visual overhaul', now - 40_000),
    session('chat-2', 'release notes format', now - 6 * M),
    session('desk-ops', 'ops-desk', now - 65 * M, { desk: 'ops-desk' }),
    session('desk-research', 'research', now - 58 * M, { desk: 'research' }),
    session('desk-builder', 'builder', now - 40 * M, { desk: 'builder' }),
    session('desk-analyst', 'analyst', now - 6 * M, { desk: 'analyst' }),
    ...Array.from({ length: 24 }, (_, i) => session(`child-${i}`, `subagent ${i}`, today(1 + i * 0.75), { parent: 'chat-1', name: `w${i}` })),
    session('old-chat', 'last week', now - 5 * 24 * H),
  ]
  let id = 9000
  const row = (ts_ms: number, kind: ActivityEntry['kind'], actor: string, subject: string, summary: string, cost: number | null = null): ActivityEntry => ({ id: id++, ts_ms, kind, actor, subject, summary, cost })
  const activityToday: ActivityEntry[] = [
    row(today(19.8), 'task_done', 'agent:ops-desk', T('1212'), 'finished t-1212 · 3 files, tests green', 0.42),
    row(today(18.5), 'task_done', 'agent:ops-desk', T('1207'), 'finished t-1207 · 3 files'),
    row(today(17.2), 'task_done', 'agent:research', T('1201'), 'landed main'),
    row(today(14.1), 'child_returned', 'agent:research', T('1199'), 'artifact report.pdf'),
    row(today(12.4), 'task_done', 'agent:builder', T('1195'), 'sandbox image built'),
    row(today(10.0), 'escalation_answered', 'user', T('1190'), 'knowledge +2'),
    row(now - 18 * M, 'routine_fired', 'system', 'nightly-digest', 'nightly-digest fired · running'),
    row(today(8.3), 'task_done', 'agent:analyst', T('1180'), 'mesh relay reachable'),
    row(today(6.0), 'routine_fired', 'system', 'morning-brief', 'morning-brief ran'),
    row(today(9.0), 'routine_fired', 'system', 'standup-notes', 'standup-notes ran'),
    row(today(12.5), 'routine_fired', 'system', 'lunch-digest', 'lunch-digest ran'),
    row(now - 12 * M, 'escalation_raised', 'agent:builder', 'approval:ap-4821', 'asks approval · cargo publish --dry-run'),
    row(now - 13 * M, 'task_checked_out', 'agent:ops-desk', T('1214'), 'took t-1214 · wire HUD scene'),
    row(now - 41 * M, 'task_checked_out', 'agent:research', T('1210'), 'took t-1210 · design 126 review'),
    row(now - 56 * M, 'child_spawned', 'agent:builder', T('1213'), 'spawned w3 · punch card arrivals'),
  ]
  // ~180 quiet rows (comments, moves, nudges) across the day: the grey field.
  const r = rng('fixture-field')
  const actors = ['agent:ops-desk', 'agent:research', 'agent:builder', 'agent:analyst', 'user', 'system']
  const quiet: ActivityEntry['kind'][] = ['task_commented', 'task_moved', 'task_nudged', 'task_created', 'task_reassigned']
  const hoursSoFar = Math.max(0.5, (now - midnight) / H)
  for (let i = 0; i < 180; i++) {
    const h = Math.min(hoursSoFar - 0.01, 0.5 + r() * hoursSoFar)
    activityToday.push(row(midnight + h * H, pick(quiet, r()), pick(actors, r()), T('1150'), `note ${i}`))
  }
  activityToday.sort((a, b) => b.ts_ms - a.ts_ms)
  // A week of arrivals, dense in working hours.
  const activityWeek: ActivityEntry[] = []
  const rw = rng('fixture-week')
  for (let i = 0; i < 480; i++) {
    const back = rw() * 7 * 24 * H
    const ts = now - back
    const hour = new Date(ts).getHours()
    if (hour < 8 || hour > 20 ? rw() < 0.15 : true) activityWeek.push(row(ts, 'task_commented', pick(actors, rw()), T('1150'), `week ${i}`))
  }
  activityWeek.sort((a, b) => b.ts_ms - a.ts_ms)
  return { attention, agents, tasks, routines, sessions, activityToday, activityWeek, activity: activityToday.slice(0, 44) }
}
