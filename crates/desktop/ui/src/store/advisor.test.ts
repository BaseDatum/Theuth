import { expect, test } from 'vitest'
import type { Advisory } from '@/rpc/bindings/Advisory'
import type { ContextEntry } from '@/rpc/bindings/ContextEntry'
import type { WireEvent } from '@/rpc/events'
import type { AgentInfo } from '@/rpc/extra'
import { apply, empty, fromHistory, type Ctx } from './reduce'
import { useSessions } from './sessions'

const S = 'sess'
const T = 'turn'
const ctx = (): Ctx => ({ sent: new Set(), now: 0 })
const msg = (seq: number): ContextEntry => ({ target: { kind: 'message', seq }, active: true, removed: false, remove_with: [], attachments: [] })
const advisory = (...notes: Advisory['notes']): Advisory => ({ advisor: 'adv', step: 3, turn: 1, at_ms: 1_700_000_000_000, notes, kind: 'review' })
const concern = { severity: 'concern' as const, note: 'the migration drops a column' }
const nit = { severity: 'nit' as const, note: 'prefer `let`' }
const WRAPPED = '<advisory severity="concern" guidance="weigh, don\'t blindly obey">\nthe migration drops a column\n</advisory>'

function run(events: WireEvent[], c = ctx()) {
  return events.reduce((s, e) => apply(s, e, c), empty(S))
}

test('a history advisory row is one advisory card with its notes, seq and time', () => {
  const [card, ...rest] = fromHistory({ role: 'advisory', advisory: advisory(concern, nit), context: msg(7) })
  expect(rest).toEqual([])
  expect(card).toMatchObject({ kind: 'advisory', notes: [concern, nit], seq: 7, at: 1_700_000_000_000 })
})

test('AdvisoryAdded appends once per seq, above a streaming tail', () => {
  const added: WireEvent = { type: 'AdvisoryAdded', session: S, seq: 7, advisory: advisory(concern) }
  const s = run([
    { type: 'TurnStarted', session: S, turn: T, text: 'hi', model: 'm', turn_seq: 1 },
    { type: 'TextDelta', session: S, turn: T, text: 'Hel' },
    added,
    added,
  ])
  expect(s.entries.map((e) => e.kind)).toEqual(['user', 'advisory', 'assistant'])
  expect(s.entries[1]).toMatchObject({ kind: 'advisory', seq: 7, notes: [concern] })
})

test('an advisory TurnStarted is never echoed as a you bubble; the event supplies the card', () => {
  const s = run([
    { type: 'TurnStarted', session: S, turn: T, text: WRAPPED, model: 'm', turn_seq: 9 },
    { type: 'AdvisoryAdded', session: S, seq: 9, advisory: advisory(concern) },
  ])
  expect(s.entries.map((e) => e.kind)).toEqual(['advisory'])
  expect(s.running).toBe(true)
})

test('a steer injected with advisory text shows one card, stamped by the event whichever lands first', () => {
  const s = run([
    { type: 'TurnStarted', session: S, turn: T, text: 'go', model: 'm', turn_seq: 1 },
    { type: 'InputQueued', session: S, id: 'q1', text: WRAPPED, defer: false },
    { type: 'InputInjected', session: S, turn: T, id: 'q1' },
    { type: 'AdvisoryAdded', session: S, seq: 4, advisory: advisory(concern) },
  ])
  expect(s.entries.filter((e) => e.kind === 'advisory')).toHaveLength(1)
  expect(s.entries.find((e) => e.kind === 'advisory')).toMatchObject({ seq: 4, notes: [concern] })
  expect(s.entries.some((e) => e.kind === 'user' && e.text.includes('<advisory'))).toBe(false)
})

test('a history user row still carrying the wrapper (older engine) parses to one card without a seq', () => {
  const entries = fromHistory({ role: 'user', text: `${WRAPPED}\n<advisory severity="nit" guidance="x">prefer \`let\`</advisory>`, turn: null })
  expect(entries).toHaveLength(1)
  expect(entries[0]).toMatchObject({ kind: 'advisory', notes: [concern, nit] })
})

test('AdvisorChanged prints one info line per model change, nothing for state ticks', () => {
  const status = (state: 'idle' | 'reviewing' | 'off', model: string | null) => ({
    session: S,
    advisor: model ? 'adv' : null,
    state,
    model,
    effort: null,
    notes: { nit: 0, concern: 0, blocker: 0 },
    context_used: 0,
    context_limit: 0,
    tokens_in: 0,
    tokens_out: 0,
    cost: null,
    step: 0,
    backlog: 0,
    last_note: null,
    detail: null,
    consults: { turn: 0, total: 0, cap: 6 },
    warning: null,
  })
  const s = run([
    { type: 'AdvisorChanged', session: S, status: status('idle', 'openai/gpt-5') },
    { type: 'AdvisorChanged', session: S, status: status('reviewing', 'openai/gpt-5') },
    { type: 'AdvisorChanged', session: S, status: status('idle', 'openai/gpt-5') },
    { type: 'AdvisorChanged', session: S, status: status('off', null) },
  ])
  expect(s.entries.map((e) => e.kind === 'info' && e.text)).toEqual(['advisor → openai/gpt-5', 'advisor → off'])
})

test('setChildren drops an advisor child: no tree row, no strip dot', () => {
  const row = (id: string, kind?: 'chat' | 'advisor'): AgentInfo => ({
    id,
    name: id,
    parent: S,
    depth: 1,
    agent: null,
    model: 'm',
    state: { state: 'running' },
    started_at: null,
    finished_at: null,
    last: '',
    kind,
  })
  useSessions.getState().setChildren(S, [row('worker'), row('advisor', 'advisor')])
  expect(Object.keys(useSessions.getState().sessions[S]?.children ?? {})).toEqual(['worker'])
})
