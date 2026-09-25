import { expect, test } from 'vitest'
import type { AgentSummary } from '@/rpc/bindings/AgentSummary'
import type { Task } from '@/rpc/bindings/Task'
import { USER, actionsFor, age, agent, awaitsMyReview, columns, defaultReviewer, needsPerson, ownerText, parseBlocker, parseOwner } from './helpers'

const task = (t: Partial<Task>): Task => ({
  id: 'x', title: 't', brief: '', owner: USER, state: 'todo', reviewer: null, blockers: [], blocks: [], parent: null, project: null,
  where: null, harness: null, external: [], thread: null, checkout: null, schedule: null, created_by: 'user',
  created_ms: 0, updated_ms: 0, no_progress_count: 0, next_wake_after_ms: null, ...t,
})
const summary = (name: string, reports_to: string): AgentSummary => ({
  file: { name, title: name, reports_to, model: null, effort: null, tools: null, mcp: [], budget: { type: 'none' }, capabilities: null, paused: false, proposed_by: null, body: 'x' },
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
})

test('queue predicates: human blockers and my reviews only', () => {
  expect(needsPerson(task({ state: 'blocked', blockers: ['person:sky'] }))).toBe(true)
  expect(needsPerson(task({ state: 'blocked', blockers: ['0192'] }))).toBe(false)
  expect(needsPerson(task({ state: 'doing', blockers: ['decision:1'] }))).toBe(false)
  expect(awaitsMyReview(task({ state: 'review', owner: agent('ops'), reviewer: USER }))).toBe(true)
  expect(awaitsMyReview(task({ state: 'review', owner: agent('ops'), reviewer: agent('lead') }))).toBe(false)
  expect(awaitsMyReview(task({ state: 'review', owner: USER }))).toBe(true)
})

test('mark done is gated by the reviewer; cancel never from review', () => {
  expect(actionsFor(task({ state: 'review', owner: agent('ops'), reviewer: agent('lead') }))).toMatchObject({ markDone: true, canMarkDone: false, cancel: false })
  expect(actionsFor(task({ state: 'review', owner: agent('ops'), reviewer: USER })).canMarkDone).toBe(true)
  expect(actionsFor(task({ state: 'doing' }))).toMatchObject({ sendBack: false, cancel: true })
  expect(actionsFor(task({ state: 'done' })).cancel).toBe(false)
})

test('columns hide cancelled unless asked and keep order', () => {
  const list = [task({ id: 'a', state: 'done' }), task({ id: 'b', state: 'cancelled' }), task({ id: 'c', state: 'todo' })]
  expect(columns(list, false).map((c) => c.state)).toEqual(['todo', 'doing', 'review', 'blocked', 'done'])
  expect(columns(list, true).at(-1)).toEqual({ state: 'cancelled', tasks: [list[1]] })
})

test('age buckets', () => {
  expect(age(0, 30_000)).toBe('30s')
  expect(age(0, 5 * 60_000)).toBe('5m')
  expect(age(0, 3 * 3_600_000)).toBe('3h')
  expect(age(0, 72 * 3_600_000)).toBe('3d')
})
