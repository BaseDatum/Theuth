import { receiveComponent } from '@/rpc/bindings/receive'
import { describe, expect, test } from 'vitest'
import type { Artifact } from "@/rpc/bindings/method.artifact.output"
import type { DocSummary } from "@/rpc/bindings/method.agent.output"
import type { HistorySearchHit } from "@/rpc/bindings/method.rpc_2dcompose.output"
import type { MeetingSummary } from "@/rpc/bindings/method.meeting.output"
import type { Meta } from "@/rpc/bindings/method.knowledge.output"
import type { SessionSummary } from "@/rpc/bindings/method.agent.output"
import { type AttachRef, atTrigger, chatName, cleanTitle, compose, marker, parse, size, subtitle, title, toRef, whiteboardRef } from './attach'
// One grammar, one fixture (66 §3.3): the same file drives `agent::attach::tests`.
import fixtures from '../../../../agent/tests/fixtures/attach_markers.json'

/** `canonical: false` marks an input `marker()` would spell differently (an unknown escape, ` · ` with empty meta). */
type Case = { name: string; input: string; refs: AttachRef[]; body: string; canonical?: boolean }
const cases = (fixtures as { cases: Case[] }).cases

describe('grammar fixtures (shared with agent::attach)', () => {
  test.each(cases.map((c) => [c.name, c] as const))('%s', (_, c) => {
    expect(parse(c.input)).toEqual({ refs: c.refs, body: c.body })
  })

  test('every parsed new-form line re-serialises to the same line', () => {
    for (const c of cases) {
      if (c.refs.length === 0 || c.input.startsWith('[doc ') || c.canonical === false) continue
      const lines = c.input.split('\n').slice(0, c.refs.length)
      c.refs.forEach((r, i) => {
        if (lines[i]?.startsWith('[attached ')) expect(marker(r)).toBe(lines[i])
      })
    }
  })
})

test('marker round-trips every kind, with `"`, `\\` and `·` in titles', () => {
  const refs: AttachRef[] = [
    { kind: 'note', id: 'agqmlbgmk57ybm2rdgyj7kcery', title: 'Groceries', meta: 'updated Sep 24 · 1 KB' },
    { kind: 'knowledge', id: 'facts/x@3f2a9c1', title: 'Say "hi" \\ bye · now', meta: 'revision 3f2a9c1 · Sep 12' },
    { kind: 'artifact', id: 'toolu_01HZ3v7Q', title: 'Q3 "pipeline" report', meta: '' },
    { kind: 'session', id: '0192a4c1-7e3b-7c0e-9c1f-2b6d1e8f4a10', title: 'Relay · reconnect', meta: 'chat · updated Sep 19' },
    { kind: 'meeting', id: '0192b0f2-0a1d-7e55-8d3a-6f2c9e1b7d44', title: 'C:\\weekly\\planning', meta: 'Sep 23 · 42 min' },
  ]
  for (const r of refs) expect(parse(marker(r))).toEqual({ refs: [r], body: '' })
  const body = 'fold it in\n\n[attached note x "not a marker"]\nthe end'
  expect(parse(compose(refs, body))).toEqual({ refs, body })
  expect(compose([], body)).toBe(body)
})

test('legacy [doc …] parses as note/knowledge; new sends use the new form', () => {
  expect(parse('[doc abc "Plan \\"B\\""]\n[doc knowledge:facts/x "Facts"]\nadd a bullet')).toEqual({
    refs: [
      { kind: 'note', id: 'abc', title: 'Plan "B"', meta: '' },
      { kind: 'knowledge', id: 'facts/x', title: 'Facts', meta: '' },
    ],
    body: 'add a bullet',
  })
  expect(marker({ kind: 'note', id: 'abc', title: 'Plan "B"', meta: '' })).toBe('[attached note abc "Plan \\"B\\""]')
})

test('218 §1: the whiteboard marker parses, round-trips and counts cards in the singular', () => {
  const id = '01a2f3c4-5b6d-7e8f-9a0b-1c2d3e4f5a6b'
  const line = `[attached whiteboard ${id} "Attention inbox" · 5 cards]`
  const ref = { kind: 'whiteboard', id, title: 'Attention inbox', meta: '5 cards' } as const
  expect(parse(`${line}\nwhich mock should we keep?`)).toEqual({ refs: [ref], body: 'which mock should we keep?' })
  expect(whiteboardRef(id, 'Attention inbox', 5)).toEqual(ref)
  expect(marker(whiteboardRef(id, 'Attention inbox', 5))).toBe(line)
  expect(marker(whiteboardRef(id, 'Attention inbox', 1))).toBe(`[attached whiteboard ${id} "Attention inbox" · 1 card]`)
  expect(marker(whiteboardRef(id, 'Attention inbox', 0))).toBe(`[attached whiteboard ${id} "Attention inbox" · 0 cards]`)
  expect(whiteboardRef(id, '', 2).title).toBe('Untitled whiteboard')
})

const NOW = new Date(2026, 8, 24, 12, 0, 0)
const doc: DocSummary = {
  id: 'agqmlbgmk57ybm2rdgyj7kcery',
  kind: 'note',
  title: 'Groceries',
  binding: 'note',
  created_ms: new Date(2026, 8, 20, 9, 0).getTime(),
  updated_ms: new Date(2026, 8, 24, 9, 0).getTime(),
  bytes: 1024,
  project: null,
}
const meta: Meta = {
  id: 'decisions/prod-dr',
  type: 'decision',
  title: 'ICD prod DR — scale to zero',
  description: '',
  tags: [],
  scope: null,
  source: null,
  timestamp: new Date(2026, 8, 12).toISOString(),
  owner: 'user',
  visibility: 'shared',
  status: 'tidy',
  hint: true,
  evidence: [],
  verified: null,
  valid_until: null,
  superseded_by: null,
  merged_into: null,
}
const revision: HistorySearchHit = {
  id: 'decisions/prod-dr',
  title: 'ICD prod DR — scale to zero',
  commit: '3f2a9c1e00000000000000000000000000000000',
  parent: null,
  time: new Date(2026, 8, 12).toISOString(),
  message: 'm',
  status: 'modified',
  side: 'after',
  snapshot_commit: null,
  match: { field: 'body', snippet: '', ranges: [] },
}
const artifact: Artifact = {
  id: 'sha',
  session: 's1',
  call_id: 'toolu_01HZ3v7Q',
  name: 'report.pdf',
  title: 'Q3 pipeline report',
  kind: 'pdf',
  mime: 'application/pdf',
  bytes: 86016,
  version: 2,
  path: '/p',
  source: null,
  preview: null,
  pages: 3,
  origin: null,
  created_at: Math.floor(new Date(2026, 8, 21).getTime() / 1000),
}
const session: SessionSummary = receiveComponent("method.agent.output.SessionSummary", {
  id: '0192a4c1-7e3b-7c0e-9c1f-2b6d1e8f4a10',
  created_at: new Date(2026, 8, 18, 14, 5).toISOString(),
  updated_at: new Date(2026, 8, 19).toISOString(),
  title: 'Fix relay reconnect loop',
  tags: [],
  model: 'm',
  parent: null,
  name: null,
  effort: null,
  agent: null,
  cwd: null,
  pinned: false,
  archived: false,
  project: 'theuth',
  notebook: null,
  retention: 'durable',
  running: false,
  owner: '',
  epoch: 1,
  memory: 'epochs',
  here: true,
})
const meeting: MeetingSummary = {
  id: '0192b0f2-0a1d-7e55-8d3a-6f2c9e1b7d44',
  started_at: new Date(2026, 8, 23, 10, 0).toISOString(),
  ended_at: null,
  title: 'Weekly planning',
  session: 's1',
  process_name: 'zoom.us',
  duration_ms: 42 * 60000,
  state: 'Done',
  project: null,
}

describe('§8 naming table', () => {
  test('note', () => {
    expect(title({ kind: 'note', doc }, NOW)).toBe('Groceries')
    expect(subtitle({ kind: 'note', doc }, NOW)).toBe('note · updated Sep 24 · 1 KB')
    expect(title({ kind: 'note', doc: { ...doc, title: '' } }, NOW)).toBe('Untitled note · Sep 20')
    expect(toRef({ kind: 'note', doc }, NOW)).toEqual({
      kind: 'note',
      id: doc.id,
      title: 'Groceries',
      meta: 'note · updated Sep 24 · 1 KB',
    })
  })
  test('knowledge and a revision', () => {
    expect(title({ kind: 'knowledge', meta }, NOW)).toBe('ICD prod DR — scale to zero')
    expect(subtitle({ kind: 'knowledge', meta }, NOW)).toBe('decision · decisions/prod-dr · Sep 12')
    expect(title({ kind: 'knowledge', meta: { ...meta, title: '' } }, NOW)).toBe('decisions/prod-dr')
    const r = toRef({ kind: 'knowledge', revision }, NOW)
    expect(r.id).toBe(`decisions/prod-dr@${revision.commit}`)
    expect(r.meta).toBe('revision 3f2a9c1 · Sep 12')
  })
  test('artifact', () => {
    expect(title({ kind: 'artifact', artifact }, NOW)).toBe('Q3 pipeline report')
    expect(title({ kind: 'artifact', artifact: { ...artifact, title: '' } }, NOW)).toBe('report.pdf')
    expect(subtitle({ kind: 'artifact', artifact }, NOW)).toBe('pdf · v2 · 3 pages · 84 KB · Sep 21')
    expect(subtitle({ kind: 'artifact', artifact, sessionTitle: 'Q3 planning' }, NOW)).toBe(
      'pdf · v2 · 3 pages · 84 KB · Sep 21 · in "Q3 planning"',
    )
    expect(toRef({ kind: 'artifact', artifact }, NOW).id).toBe('toolu_01HZ3v7Q')
  })
  test('session', () => {
    expect(title({ kind: 'session', session }, NOW)).toBe('Fix relay reconnect loop')
    expect(title({ kind: 'session', session: { ...session, title: '' } }, NOW)).toBe('Untitled chat · Sep 18')
    expect(subtitle({ kind: 'session', session }, NOW)).toBe('chat · theuth · updated Sep 19')
    expect(subtitle({ kind: 'session', session: { ...session, project: null, archived: true } }, NOW)).toBe(
      'chat · no project · updated Sep 19 · archived',
    )
  })
  test('meeting', () => {
    expect(title({ kind: 'meeting', meeting }, NOW)).toBe('Weekly planning')
    expect(title({ kind: 'meeting', meeting: { ...meeting, title: null } }, NOW)).toBe('zoom.us')
    expect(title({ kind: 'meeting', meeting: { ...meeting, title: null, process_name: '' } }, NOW)).toBe('Meeting · Sep 23')
    expect(subtitle({ kind: 'meeting', meeting }, NOW)).toBe('Sep 23 · 42 min')
    expect(subtitle({ kind: 'meeting', meeting: { ...meeting, state: 'Recording', duration_ms: null } }, NOW)).toBe('Sep 23 · recording')
  })
  test('dates outside this year carry the year; sizes keep one decimal below 10', () => {
    expect(subtitle({ kind: 'note', doc: { ...doc, updated_ms: new Date(2025, 0, 3).getTime(), bytes: 1536 } }, NOW)).toBe(
      'note · updated Jan 3, 2025 · 1.5 KB',
    )
    expect(size(512)).toBe('512 B')
    expect(size(10 * 1024)).toBe('10 KB')
    expect(size(3.5 * 1024 * 1024)).toBe('3.5 MB')
  })
})

test('@ trigger: word boundary only', () => {
  expect(atTrigger('a@b', 3)).toBeNull()
  expect(atTrigger('sky@x.com', 9)).toBeNull()
  expect(atTrigger(' @q', 3)).toEqual({ start: 1, word: 'q' })
  expect(atTrigger('@', 1)).toEqual({ start: 0, word: '' })
  expect(atTrigger('@gro', 4)).toEqual({ start: 0, word: 'gro' })
  expect(atTrigger('hello @gro', 10)).toEqual({ start: 6, word: 'gro' })
  // A space ends the word: the text stays as typed and nothing is triggered.
  expect(atTrigger('@gro ', 5)).toBeNull()
  // The caret decides: before the `@` there is no trigger.
  expect(atTrigger('@gro', 0)).toBeNull()
  expect(atTrigger('line one\n@x', 11)).toEqual({ start: 9, word: 'x' })
})

test('183 R2: a chat seeded from a peer message never shows the wrapper', () => {
  const cut = '<message from="session:01a1be05-ccc8-737a-bd90-33d944959adf…'
  expect(chatName({ title: cut, created_at: '2026-11-09T01:55:00Z' })).toBe('Message from another agent')
  expect(cleanTitle('<message from="peer:xeus" id="x">\nhello\n</message>')).toBe('Message from xeus: hello')
  expect(cleanTitle('Message from ops: P2 ping')).toBe('Message from ops: P2 ping')
  expect(cleanTitle('Gateway limits')).toBe('Gateway limits')
})

test('a chat seeded from a schedule fire is named for its schedule, never the wrapper', () => {
  expect(cleanTitle('<schedule name="dreaming" at="2026-11-03T02:00:00+00:00" qu…')).toBe('dreaming')
  expect(cleanTitle('<schedule name="morning-br…')).toBe('morning-br')
  expect(chatName({ title: '<schedule name="" at="x">', created_at: '2026-11-09T01:55:00Z' })).toBe('<schedule name="" at="x">')
})
