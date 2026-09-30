import { expect, test } from 'vitest'
import type { AgentSummary } from '@/rpc/bindings/AgentSummary'
import type { Task } from '@/rpc/bindings/Task'
import { USER, actionsFor, age, agent, awaitsMyReview, canDrag, columns, defaultReviewer, needsPerson, nodesOf, ownerText, parseBlocker, parseOwner, stateChip } from './helpers'

const task = (t: Partial<Task>): Task => ({
  id: 'x', title: 't', brief: '', done_when: null, attachments: [], owner: USER, state: 'todo', reviewer: null, blockers: [], blocks: [], parent: null, project: null,
  where: null, harness: null, external: [], thread: null, checkout: null, schedule: null, created_by: 'user',
  created_ms: 0, updated_ms: 0, no_progress_count: 0, next_wake_after_ms: null, ...t,
})
const summary = (name: string, reports_to: string): AgentSummary => ({
  file: { name, title: name, reports_to, model: null, effort: null, tools: null, mcp: null, budget: { type: 'none' }, capabilities: null, paused: false, proposed_by: null, runs_in: 'any', body: 'x' },
  status: 'idle', desk: null, open_tasks: 0, spent_month: 0,
})

test('owner text round-trips both forms', () => {
  expect(ownerText(agent('ops'))).toBe('agent:ops')
  expect(parseOwner('agent:ops')).toEqual(agent('ops'))
  expect(parseOwner('user')).toEqual(USER)
  expect(parseOwner('theuth')).toEqual(USER)
})

test('default reviewer follows reports_to up to the user', () => {
  const agents = [summary('ops', 'theuth'), summary('writer', 'ops')]
  expect(defaultReviewer(agent('writer'), agents)).toEqual(agent('ops'))
  expect(defaultReviewer(agent('ops'), agents)).toEqual(USER)
  expect(defaultReviewer(agent('ghost'), agents)).toEqual(USER)
  expect(defaultReviewer(USER, agents)).toEqual(USER)
})

test('blockers parse by prefix; a bare id is a task', () => {
  expect(parseBlocker('0192-abc').kind).toBe('task')
  expect(parseBlocker('decision:apr-1')).toMatchObject({ kind: 'decision', id: 'apr-1' })
  expect(parseBlocker('person:sky')).toMatchObject({ kind: 'person', id: 'sky' })
  expect(parseBlocker('engine:build-B').kind).toBe('other')
  expect(parseBlocker('no continuation').kind).toBe('other')
})

test('queue predicates: human blockers and my reviews only', () => {
  expect(needsPerson(task({ state: 'blocked', blockers: ['person:sky'] }))).toBe(true)
  expect(needsPerson(task({ state: 'blocked', blockers: ['0192'] }))).toBe(false)
  expect(needsPerson(task({ state: 'doing', blockers: ['decision:1'] }))).toBe(false)
  expect(awaitsMyReview(task({ state: 'review', owner: agent('ops'), reviewer: USER }))).toBe(true)
  expect(awaitsMyReview(task({ state: 'review', owner: agent('ops'), reviewer: agent('lead') }))).toBe(false)
  expect(awaitsMyReview(task({ state: 'review', owner: USER }))).toBe(true)
})

test('mark done is gated by the reviewer; cancel never from review; start and pull back are backlog ↔ todo', () => {
  expect(actionsFor(task({ state: 'review', owner: agent('ops'), reviewer: agent('lead') }))).toMatchObject({ markDone: true, canMarkDone: false, cancel: false })
  expect(actionsFor(task({ state: 'review', owner: agent('ops'), reviewer: USER })).canMarkDone).toBe(true)
  expect(actionsFor(task({ state: 'doing' }))).toMatchObject({ sendBack: false, cancel: true, start: false, toBacklog: false })
  expect(actionsFor(task({ state: 'done' })).cancel).toBe(false)
  expect(actionsFor(task({ state: 'backlog' }))).toMatchObject({ start: true, toBacklog: false, cancel: true })
  expect(actionsFor(task({ state: 'todo' }))).toMatchObject({ start: false, toBacklog: true })
  expect(actionsFor(task({ state: 'blocked', blockers: ['no continuation'] })).unblock).toBe(true)
  expect(actionsFor(task({ state: 'blocked', blockers: ['no continuation', 'decision:d1'] })).unblock).toBe(false)
  expect(actionsFor(task({ state: 'doing' })).unblock).toBe(false)
})

test('columns start with backlog, hide cancelled unless asked and keep order', () => {
  const list = [task({ id: 'a', state: 'done' }), task({ id: 'b', state: 'cancelled' }), task({ id: 'c', state: 'todo' }), task({ id: 'd', state: 'backlog' })]
  expect(columns(list, false).map((c) => c.state)).toEqual(['backlog', 'todo', 'doing', 'review', 'blocked', 'done'])
  expect(columns(list, false)[0]).toEqual({ state: 'backlog', tasks: [list[3]] })
  expect(columns(list, true).at(-1)).toEqual({ state: 'cancelled', tasks: [list[1]] })
})

test('state chip text follows the rulings: not started, queued for a desk, at a desk only with an open session', () => {
  const checkout = { session: 's', engine: 'mac', epoch: 1, progress_at_ms: 0, orphaned: false, taken: false }
  expect(stateChip(task({ state: 'backlog', owner: agent('ops') }))).toBe('not started')
  expect(stateChip(task({ state: 'todo', owner: agent('ops') }))).toBe("queued for ops's desk")
  expect(stateChip(task({ state: 'todo', owner: USER }))).toBe('todo')
  expect(stateChip(task({ state: 'doing', owner: agent('ops'), checkout }))).toBe("at ops's desk · open session")
  expect(stateChip(task({ state: 'doing', owner: agent('ops') }))).toBe("queued for ops's desk")
  expect(stateChip(task({ state: 'doing', owner: USER, checkout }))).toBe('in progress · open session')
  expect(stateChip(task({ state: 'doing', owner: USER }))).toBe('in progress')
  expect(stateChip(task({ state: 'review' }))).toBe('review')
  expect(stateChip(task({ state: 'blocked' }))).toBe('blocked')
})

test('drag is backlog ↔ todo only', () => {
  expect(canDrag('backlog', 'todo')).toBe(true)
  expect(canDrag('todo', 'backlog')).toBe(true)
  expect(canDrag('todo', 'todo')).toBe(false)
  expect(canDrag('todo', 'doing')).toBe(false)
  expect(canDrag('review', 'todo')).toBe(false)
})

test('nodes list this node first, then peers, without repeating self', () => {
  const peer = (name: string, reachable: boolean) => ({ id: name, name, addrs: [], reachable, last_seen_ms: 0, hub_pref: 0 })
  const mesh = { self_id: 'me', self_name: 'mac', mesh_name: null, peers: [peer('mac', true), peer('build-A', true), peer('nas', false)], lease: null, traffic: null, replica: null }
  expect(nodesOf(mesh as never).map((n) => [n.name, n.here, n.reachable])).toEqual([
    ['mac', true, true],
    ['build-A', false, true],
    ['nas', false, false],
  ])
  expect(nodesOf(undefined)).toEqual([])
})

test('age buckets', () => {
  expect(age(0, 30_000)).toBe('30s')
  expect(age(0, 5 * 60_000)).toBe('5m')
  expect(age(0, 3 * 3_600_000)).toBe('3h')
  expect(age(0, 72 * 3_600_000)).toBe('3d')
})
