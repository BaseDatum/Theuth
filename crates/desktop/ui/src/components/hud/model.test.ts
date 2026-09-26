import { expect, test } from 'vitest'
import type { ActivityEntry } from '@/rpc/bindings/ActivityEntry'
import type { AgentSummary } from '@/rpc/bindings/AgentSummary'
import type { AttentionItem } from '@/rpc/bindings/AttentionItem'
import type { Task } from '@/rpc/bindings/Task'
import { appendActivity, badgeCount, boardPulse, headline, inFlight, snoozeUntil, spend } from './model'

const agent = (name: string, extra: Partial<AgentSummary> = {}, file: Partial<AgentSummary['file']> = {}): AgentSummary => ({
  file: { name, title: name, reports_to: 'theuth', model: null, effort: null, tools: null, mcp: [], budget: { type: 'none' }, capabilities: null, paused: false, proposed_by: null, body: 'x', ...file },
  status: 'idle',
  desk: null,
  open_tasks: 0,
  spent_month: 0,
  ...extra,
})

const task = (id: string, state: Task['state'], extra: Partial<Task> = {}): Task => ({
  id,
  title: id,
  brief: '',
  done_when: null,
  attachments: [],
  owner: { type: 'agent', name: 'ops' },
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
  checkout: null,
  schedule: null,
  created_by: 'user',
  created_ms: 1,
  updated_ms: 10,
  no_progress_count: 0,
  next_wake_after_ms: null,
  ...extra,
})

const item = (id: string): AttentionItem => ({ id, kind: 'blocked', title: id, detail: null, at_ms: 1, task: null, agent: null, session: null, approval: null, decision: null })

const row = (id: number): ActivityEntry => ({ id, ts_ms: id, kind: 'task_moved', actor: 'user', subject: 't', summary: `${id}`, cost: null })

test('headline counts active agents, doing tasks, month spend and pending attention', () => {
  const agents = [
    agent('theuth', { status: 'working' }),
    agent('ops', { status: 'idle', spent_month: 12.5 }),
    agent('amy', { status: 'proposed' }, { proposed_by: 'agent:ops', paused: true }),
    agent('cap', { status: 'paused', spent_month: 10 }, { paused: true }),
  ]
  const tasks = [task('a', 'doing'), task('b', 'doing'), task('c', 'review'), task('d', 'done')]
  expect(headline(agents, tasks, [item('x'), item('y')])).toEqual({ agentsActive: 2, tasksInProgress: 2, monthSpend: 22.5, pending: 2 })
})

test('badge is the inbox length and hides at zero', () => {
  expect(badgeCount(undefined)).toBeNull()
  expect(badgeCount([])).toBeNull()
  expect(badgeCount([item('a'), item('b'), item('c')])).toBe(3)
})

test('live rows prepend newest-first, dedupe by id and cap the list', () => {
  const list = [row(3), row(2), row(1)]
  expect(appendActivity(list, row(4)).map((e) => e.id)).toEqual([4, 3, 2, 1])
  expect(appendActivity(list, row(2)).map((e) => e.id)).toEqual([3, 2, 1])
  // A late row lands in order, not on top.
  expect(appendActivity([row(5), row(3)], row(4)).map((e) => e.id)).toEqual([5, 4, 3])
  expect(appendActivity(list, row(9), 2).map((e) => e.id)).toEqual([9, 3])
})

test('in flight lists doing tasks longest-running first with their checkout', () => {
  const tasks = [
    task('fresh', 'doing', { checkout: { session: 's2', engine: 'mac', epoch: 1, progress_at_ms: 900, orphaned: false, taken: false } }),
    task('old', 'doing', { checkout: { session: 's1', engine: 'build-a', epoch: 2, progress_at_ms: 100, orphaned: false, taken: false } }),
    task('mine', 'doing', { owner: { type: 'user' }, updated_ms: 500 }),
    task('queued', 'todo'),
  ]
  const rows = inFlight(tasks)
  expect(rows.map((r) => [r.task.id, r.agent, r.session, r.engine, r.sinceMs])).toEqual([
    ['old', 'ops', 's1', 'build-a', 100],
    ['mine', 'you', null, null, 500],
    ['fresh', 'ops', 's2', 'mac', 900],
  ])
})

test('board pulse counts every open column in board order', () => {
  const tasks = [task('a', 'todo'), task('b', 'todo'), task('c', 'blocked'), task('d', 'done'), task('e', 'cancelled')]
  expect(boardPulse(tasks)).toEqual([
    { state: 'backlog', n: 0 },
    { state: 'todo', n: 2 },
    { state: 'doing', n: 0 },
    { state: 'review', n: 0 },
    { state: 'blocked', n: 1 },
    { state: 'done', n: 1 },
  ])
})

test('spend ranks agents by budget share, unbudgeted spenders after, proposed and idle-free skipped', () => {
  const agents = [
    agent('theuth'),
    agent('free', { spent_month: 3 }),
    agent('ops', { spent_month: 32 }, { budget: { type: 'monthly', amount: 40, currency: 'USD' } }),
    agent('dev', { spent_month: 1 }, { budget: { type: 'monthly', amount: 100, currency: 'USD' } }),
    agent('amy', { status: 'proposed', spent_month: 0 }, { proposed_by: 'agent:ops', budget: { type: 'monthly', amount: 5, currency: 'USD' } }),
  ]
  const rows = spend(agents)
  expect(rows.map((r) => [r.name, r.share])).toEqual([
    ['ops', 0.8],
    ['dev', 0.01],
    ['free', null],
  ])
})

test('snooze presets: an hour, 09:00 tomorrow, a week', () => {
  const now = new Date(2026, 8, 25, 22, 30).getTime()
  expect(snoozeUntil('hour', now) - now).toBe(3_600_000)
  expect(snoozeUntil('week', now) - now).toBe(7 * 86_400_000)
  expect(new Date(snoozeUntil('tomorrow', now)).toLocaleString('sv-SE')).toBe('2026-09-26 09:00:00')
})
