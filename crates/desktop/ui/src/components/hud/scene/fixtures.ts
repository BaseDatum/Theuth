import { receiveComponent } from '@/rpc/bindings/receive'
import type { ActivityEntry } from "@/rpc/bindings/method.roster.output"
import type { AgentSummary } from "@/rpc/bindings/method.roster.output"
import type { AttentionItem } from "@/rpc/bindings/method.roster.output"
import type { ScheduleInfo } from "@/rpc/bindings/method.schedule.output"
import type { SessionSummary } from "@/rpc/bindings/method.agent.output"
import type { SpendReport } from "@/rpc/bindings/method.agent.output"
import type { Task } from "@/rpc/bindings/method.roster.output"
import { DAY_MS, HOUR_MS, localDay, rng, type SceneData, startOfDay } from './model'

/**
 * Synthetic data that reproduces the approved mockup: 6 needs-you items, 6 agent rings + 4 routine rings, 9 done, 3 running tasks + 1 running routine, ~230 field lines,
 * a week of arrivals, spend rows and a stream. Timed relative to `now` so the arc reads right at any hour.
 */

const M = 60_000
const pick = <T,>(arr: readonly T[], u: number): T => arr[Math.floor(u * arr.length)] as T
const H = HOUR_MS

const agent = (name: string, status: AgentSummary['status'], open_tasks: number, spent_month: number, cap: number | null, desk: string | null): AgentSummary => (receiveComponent("method.roster.output.AgentSummary", {
  file: { name, title: name, reports_to: 'theuth', model: null, effort: null, tools: null, mcp: null, budget: cap === null ? { type: 'none' } : { type: 'monthly', amount: cap, currency: 'USD' }, capabilities: null, paused: status === 'paused', proposed_by: null, runs_in: 'any', body: '' },
  status,
  desk,
  open_tasks,
  spent_month,
}))

const task = (id: string, title: string, owner: string, state: Task['state'], updated_ms: number, progress_at_ms: number | null = null, project: string | null = null): Task => ({
  id,
  title,
  brief: '',
  owner: owner === 'user' ? { type: 'user' } : { type: 'agent', name: owner },
  state,
  reviewer: null,
  resolved_blockers: [],
  blockers: [],
  blocks: [],
  parent: null,
  accent: null,
  project,
  where: null,
  home: null,
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
  priority: 'medium',
})

const session = (id: string, title: string, updated: number, extra: Partial<SessionSummary> = {}): SessionSummary => (receiveComponent("method.agent.output.SessionSummary", {
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
  notebook: null,
  retention: 'durable',
  running: false,
  owner: '',
  epoch: 0,
  memory: 'epochs',
  here: true,
  ...extra,
}))

const schedule = (name: string, next: number | null, last: number | null, running: boolean, last_result: string | null): ScheduleInfo => (receiveComponent("method.schedule.output.ScheduleInfo", {
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
  sensor: null,
  fire: null,
  for: null,
  cooldown: null,
  mode: null,
  memory: null,
  expires: null,
  timeout: null,
  model: null,
  run_timeout: null,
  created: null,
  last_check: null,
}))

const need = (id: string, kind: AttentionItem['kind'], title: string, at_ms: number, agent: string | null, task: string | null): AttentionItem => (receiveComponent("method.roster.output.AttentionItem", {
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
  packet: null,
  ask: null,
  turn: null,
  rank: 5,
}))

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
    receiveComponent("method.roster.output.AttentionItem", { ...need('secret:gh-token', 'secret', 'GITHUB_TOKEN rotation requested', now - 50 * H, null, null), session: 'chat-1' }), // Revision 4: via the root's ring
  ]
  // Revision 6: the BOARD panel reads every task in every project — `hud` (9), `relay` (5), unfiled (6); every state present (doing 3, done 7, review 2, blocked 1, cancelled 1, backlog 4, todo 2).
  const tasks: Task[] = [
    task(T('1214'), 'wire HUD scene into /', 'ops-desk', 'doing', now - 12 * M, now - 12 * M, 'hud'),
    task(T('1210'), 'design 126 review', 'research', 'doing', now - 40 * M, now - 40 * M, 'hud'),
    task(T('1213'), 'punch card arrivals', 'builder', 'doing', now - 55 * M, now - 55 * M, 'hud'),
    task(T('1212'), 'release notes 2.4', 'ops-desk', 'done', today(19.8)),
    task(T('1207'), 'relay cert rotation', 'ops-desk', 'done', today(18.5), null, 'relay'),
    task(T('1201'), 'landed main', 'research', 'done', today(17.2), null, 'hud'),
    task(T('1199'), 'artifact report.pdf', 'research', 'done', today(14.1)),
    task(T('1195'), 'sandbox image', 'builder', 'done', today(12.4)),
    task(T('1190'), 'knowledge +2', 'user', 'done', today(10.0)),
    task(T('1180'), 'mesh relay', 'analyst', 'done', today(8.3), null, 'relay'),
    task(T('1188'), 'design 126 draft', 'research', 'review', now - 1 * H, null, 'hud'),
    task(T('1187'), 'relay failover runbook', 'ops-desk', 'review', now - 30 * H, null, 'relay'),
    task(T('1203'), 'windows build host', 'builder', 'blocked', now - 26 * H, null, 'hud'),
    task(T('1170'), 'old idea', 'user', 'cancelled', today(9.0)),
    task(T('1160'), 'backlog item', 'user', 'backlog', now - 3 * 24 * H),
    task(T('1158'), 'compact scene polish', 'user', 'backlog', now - 4 * 24 * H, null, 'hud'),
    task(T('1157'), 'relay metrics', 'analyst', 'backlog', now - 5 * 24 * H, null, 'relay'),
    task(T('1156'), 'arc labels at 1400', 'user', 'backlog', now - 6 * 24 * H, null, 'hud'),
    task(T('1172'), 'spend panel caps', 'builder', 'todo', now - 20 * H, null, 'hud'),
    task(T('1171'), 'relay cert alarm', 'ops-desk', 'todo', now - 28 * H, null, 'relay'),
  ]
  // Revision 1: morning-brief fired and finished, standup-notes fired and failed, nightly-digest running, knowledge-sync due at 23:30.
  const routines = [
    schedule('morning-brief', midnight + 30 * H, today(6.0), false, 'ok'),
    schedule('standup-notes', midnight + 33 * H, today(9.0), false, 'error: no notes'),
    schedule('nightly-digest', midnight + 42 * H, now - 18 * M, true, 'ok'),
    schedule('knowledge-sync', midnight + 23.5 * H, midnight - 0.5 * H, false, 'ok'),
  ]
  // Revision 4: four active roots — chat-1 running with 3 live children (2 running), chat-2 running alone, chat-3 idle with one
  // running child, chat-4 idle but recent — plus the not-shown kinds: an old root, an advisor, an archived chat.
  const sessions: SessionSummary[] = [
    session('chat-1', 'HUD visual overhaul', now - 40_000, { running: true }),
    session('chat-2', 'release notes format', now - 6 * M, { running: true }),
    session('chat-3', 'onboarding doc', now - 50 * M),
    session('chat-4', 'quick question', now - 12 * M),
    session('desk-ops', 'ops-desk', now - 65 * M, { desk: 'ops-desk' }),
    session('desk-research', 'research', now - 58 * M, { desk: 'research' }),
    session('desk-builder', 'builder', now - 40 * M, { desk: 'builder' }),
    session('desk-analyst', 'analyst', now - 6 * M, { desk: 'analyst' }),
    ...Array.from({ length: 21 }, (_, i) => session(`child-${i}`, `subagent ${i}`, today(1 + i * 0.75), { parent: 'chat-1', name: `w${i}` })),
    session('child-21', 'subagent 21', now - 20_000, { parent: 'chat-1', name: 'w21', running: true }),
    session('child-22', 'subagent 22', now - 2 * M, { parent: 'chat-1', name: 'w22', running: true }),
    session('child-23', 'subagent 23', now - 10 * M, { parent: 'chat-1', name: 'w23' }),
    session('child-c1', 'onboarding research', now - 1 * M, { parent: 'chat-3', name: 'c1', running: true }),
    session('old-chat', 'last week', now - 5 * 24 * H),
    session('adv-1', 'advisor', now - 1 * M, { kind: 'advisor', running: true }),
    session('arch-1', 'archived chat', now - 1 * M, { archived: true }),
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
    row(today(9.0), 'routine_fired', 'system', 'standup-notes', 'standup-notes failed · no notes'),
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
  // Rev 3: a week of ledger spend ending today (the mock's $1,302.38 today), the month to date, owners by root.
  const spendDays = [412.1, 388.4, 540.9, 0, 611.2, 849.0, 1302.38]
  const spend: SpendReport = {
    days: spendDays.map((usd, i) => ({ day: localDay(now - (6 - i) * DAY_MS), usd, calls: usd === 0 ? 0 : 300 + i * 40, unresolved: i === 6 ? 2 : 0 })),
    month: { day: localDay(now).slice(0, 7), usd: 4102.0, calls: 36_861, unresolved: 2 },
    owners: [
      { kind: 'user', name: 'you', usd: 2210.5, calls: 2104 },
      { kind: 'agent', name: 'builder', usd: 1104.2, calls: 1230 },
      { kind: 'agent', name: 'research', usd: 522.0, calls: 610 },
      { kind: 'routine', name: 'nightly-digest', usd: 148.9, calls: 160 },
      { kind: 'agent', name: 'ops-desk', usd: 118.4, calls: 90 },
    ],
  }
  // Revision 2/4: context gauges (session id → fill) for three desks and two roots; analyst's desk, chat-3 and chat-4 have none yet.
  const ctx = (used: number, window: number, live: boolean) => ({ fill: used / window, used, window, live })
  const contexts = new Map([
    ['desk-ops', ctx(124_000, 200_000, true)],
    ['desk-research', ctx(62_000, 200_000, true)],
    ['desk-builder', ctx(176_000, 200_000, false)],
    ['chat-1', ctx(142_000, 200_000, true)],
    ['chat-2', ctx(38_000, 200_000, true)],
  ])
  return { attention, agents, tasks, routines, sessions, activityToday, activityWeek, activity: activityToday.slice(0, 44), contexts, spend }
}
