// 114 + Sky's 16:55 correction: a decision ask is never a chat card (D1); every other loose prompt stays appended at
// the bottom, as before 114, so an open approval or question is never lost in the transcript after a reload.
import { expect, test } from 'vitest'
import type { ApprovalRequested } from "@/rpc/bindings/event.rpc_2estream.output"
import type { AskRequested } from "@/rpc/bindings/method.roster.output"
import type { AgentInfo } from '@/store/ui-events'
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
const approval = (id: string, tool: string, session = 'desk'): ApprovalRequested => ({ type: 'ApprovalRequested', id, session, tool, args: {}, risk: 'mutating', human_only: false })
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
  const rows = buildRows(entries, [approval('a', 'shell')], 'desk', undefined, false)
  expect(shape(rows)).toEqual(['u1', 't1', 'u2', 't2'])
  expect((rows[3] as Extract<Row, { entry: Entry }>).prompts?.map((p) => p.id)).toEqual(['a'])
})

test('decision ask is dropped from rows entirely (with and without an escalate row, own session and child session)', () => {
  const decision = (id: string, session: string) => ask(id, session, 'decision:0199-abc')
  for (const es of [entries, entries.filter((e) => e.id !== 't1')]) {
    const rows = buildRows(es, [decision('d-own', 'desk'), decision('d-kid', 'kid')], 'desk', child, false)
    expect(shape(rows)).toEqual(es.map((e) => e.id))
    expect(rows.every((r) => !('prompts' in r) || r.prompts === null)).toBe(true)
  }
})

test('loose prompts (child approval, direct ask without a row) are appended at the end in pending order', () => {
  const rows = buildRows(entries, [approval('ka', 'shell', 'kid'), ask('q1'), approval('a', 'shell')], 'desk', child, false)
  expect(shape(rows)).toEqual(['u1', 't1', 'u2', 't2', 'p:ka', 'p:q1'])
  expect((rows[4] as Extract<Row, { prompt: unknown }>).from?.id).toBe('kid')
  expect((rows[5] as Extract<Row, { prompt: unknown }>).from).toBeUndefined()
  // An unknown session (not in the subtree) is not this transcript's business.
  expect(shape(buildRows(entries, [approval('x', 'shell', 'other')], 'desk', child, false))).toEqual(['u1', 't1', 'u2', 't2'])
})

test('a prompt row carries no at; withGaps sees entries only', () => {
  const es = [user('u1', T), user('u2', T + 3 * GAP_MS)]
  const rows = buildRows(es, [ask('q')], 'desk', undefined, false)
  expect(shape(rows)).toEqual(['u1', 'gap', 'u2', 'p:q'])
  expect('at' in (rows[3] as object)).toBe(false)
})

test('buildRows passes the pinned thinking ids to foldRuns; with collapse off nothing folds either way', () => {
  const think: Entry = { id: 'r', kind: 'reasoning', text: 'hm', done: false, at: T + 15 }
  const es = [tool('t1', 'shell', T), tool('t2', 'shell', T + 10), think, tool('t3', 'shell', T + 20), tool('t4', 'shell', T + 30)]
  expect(shape(buildRows(es, [], 'desk', undefined, true))).toEqual(['run:t1'])
  expect(shape(buildRows(es, [], 'desk', undefined, true, new Set(['r'])))).toEqual(['run:t1', 'r', 'run:t3'])
  expect(shape(buildRows(es, [], 'desk', undefined, false, new Set(['r'])))).toEqual(['t1', 't2', 'r', 't3', 't4'])
})
