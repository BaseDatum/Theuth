// 114: a pending prompt is a transcript card at its moment in the flow, never anchored at the bottom; a decision ask
// is never a chat card at all (Sky's D1).
import { expect, test } from 'vitest'
import type { ApprovalRequested } from '@/rpc/bindings/ApprovalRequested'
import type { AskRequested } from '@/rpc/bindings/AskRequested'
import type { AgentInfo } from '@/rpc/extra'
import type { Entry } from '@/store/sessions'
import { GAP_MS } from '@/lib/time'
import { buildRows, type Row } from './rows'

const user = (id: string, at: number): Entry => ({ id, kind: 'user', text: id, at, turn: 1 })
const tool = (id: string, name: string, at: number, done = true): Entry => ({
  id,
  kind: 'tool',
  callId: id,
  name,
  args: {},
  pending: false,
  child: false,
  at,
  ...(done ? { result: { ok: true, output: 'done' } as never } : {}),
})
const approval = (id: string, tool: string, session = 'desk'): ApprovalRequested => ({ type: 'ApprovalRequested', id, session, tool, args: {}, risk: 'mutating' })
const ask = (id: string, session = 'desk', questionId = 'q'): AskRequested => ({
  type: 'AskRequested',
  id,
  session,
  questions: [{ id: questionId, question: 'which?', options: [] }],
})
const child: Record<string, AgentInfo> = { kid: { id: 'kid', name: 'kid' } as AgentInfo }

/** `u1`, `tool`, `p:<id>`, `gap` per row. */
const shape = (rows: Row[]) => rows.map((r) => ('entry' in r ? r.entry.id : 'run' in r ? `run:${r.run[0]!.id}` : 'prompt' in r ? `p:${r.prompt.id}` : 'gap'))

const T = 1_000_000
const entries = [user('u1', T), tool('t1', 'escalate', T + 10), user('u2', T + 20), tool('t2', 'shell', T + 30, false)]

test('approval under its own unfinished tool row, not loose (R2)', () => {
  const rows = buildRows(entries, [approval('a', 'shell')], new Map([['a', T + 31]]), 'desk', undefined, false)
  expect(shape(rows)).toEqual(['u1', 't1', 'u2', 't2'])
  expect((rows[3] as Extract<Row, { entry: Entry }>).prompts?.map((p) => p.id)).toEqual(['a'])
})

test('decision ask is dropped from rows entirely (with and without an escalate row, own session and child session)', () => {
  const decision = (id: string, session: string) => ask(id, session, 'decision:0199-abc')
  const at = new Map([
    ['d-own', T + 15],
    ['d-kid', T + 15],
  ])
  for (const es of [entries, entries.filter((e) => e.id !== 't1')]) {
    const rows = buildRows(es, [decision('d-own', 'desk'), decision('d-kid', 'kid')], at, 'desk', child, false)
    expect(shape(rows)).toEqual(es.map((e) => e.id))
    expect(rows.every((r) => !('prompts' in r) || r.prompts === null)).toBe(true)
  }
  // Without an arrival time either: still no card.
  expect(shape(buildRows(entries, [decision('d-own', 'desk')], new Map(), 'desk', child, false))).toEqual(['u1', 't1', 'u2', 't2'])
})

test('a loose ask lands by arrival time, after the last row at or before it (R3)', () => {
  const rows = buildRows(entries, [ask('q1')], new Map([['q1', T + 10]]), 'desk', undefined, false)
  expect(shape(rows)).toEqual(['u1', 't1', 'p:q1', 'u2', 't2'])
})

test('a child session prompt is placed by time and labelled with the child', () => {
  const rows = buildRows(entries, [approval('ka', 'shell', 'kid')], new Map([['ka', T + 25]]), 'desk', child, false)
  expect(shape(rows)).toEqual(['u1', 't1', 'u2', 'p:ka', 't2'])
  expect((rows[3] as Extract<Row, { prompt: unknown }>).from?.id).toBe('kid')
  // An unknown session (not in the subtree) is not this transcript's business.
  expect(shape(buildRows(entries, [approval('x', 'shell', 'other')], new Map([['x', T + 25]]), 'desk', child, false))).toEqual(['u1', 't1', 'u2', 't2'])
})

test('two loose prompts keep arrival order; one without an arrival comes last', () => {
  const at = new Map([
    ['late', T + 25],
    ['early', T + 5],
  ])
  const rows = buildRows(entries, [ask('late'), ask('early'), ask('none')], at, 'desk', undefined, false)
  expect(shape(rows)).toEqual(['u1', 'p:early', 't1', 'u2', 'p:late', 't2', 'p:none'])
})

test('a prompt row has an at, so withGaps divides around it like any entry', () => {
  const es = [user('u1', T), user('u2', T + 3 * GAP_MS)]
  const rows = buildRows(es, [ask('q')], new Map([['q', T + GAP_MS + 1]]), 'desk', undefined, false)
  expect(shape(rows)).toEqual(['u1', 'gap', 'p:q', 'gap', 'u2'])
})

test('collapsed runs: a loose prompt sits between runs by the run’s first entry time', () => {
  const es = [tool('a', 'shell', T), tool('b', 'shell', T + 1), user('u', T + 50), tool('c', 'shell', T + 60), tool('d', 'shell', T + 61)]
  const rows = buildRows(es, [ask('q')], new Map([['q', T + 55]]), 'desk', undefined, true)
  expect(shape(rows)).toEqual(['run:a', 'u', 'p:q', 'run:c'])
})
