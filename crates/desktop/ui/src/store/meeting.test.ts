import { expect, test } from 'vitest'
import type { Brief } from '@/rpc/bindings/Brief'
import type { BriefSource } from '@/rpc/bindings/BriefSource'
import type { MeetingEvent } from '@/rpc/bindings/MeetingEvent'
import type { OracleDecision } from '@/rpc/bindings/OracleDecision'
import { type MeetingSlice, LINES_KEEP, ORACLE_KEEP, current, oracleCounters, oracleSummary, reduce, useMeeting } from './meeting'

const M = 'm1'
const brief = (id: string, source: BriefSource, topic: string | null = null): Brief => ({
  id,
  topic,
  title: id,
  bullets: ['a', 'b'],
  source,
  at_ms: 0n,
  parent: null,
  cached: null,
})
const ev = (e: Omit<Extract<MeetingEvent, { type: 'Brief' }>, 'meeting'>): MeetingEvent => ({ meeting: M, ...e })

function fresh(): MeetingSlice {
  const s = useMeeting.getState()
  return reduce(s, {
    type: 'Started',
    meeting: M,
    session: 's1',
    process: { pid: 1, name: 'zoom', bundle_id: null, active: true },
    at: '2026-09-20T00:00:00Z',
  })
}

test('Auto briefs at the top replace the view; stepped back they queue behind the badge', () => {
  let s = fresh()
  for (const id of ['a', 'b', 'c']) s = reduce(s, ev({ type: 'Brief', brief: brief(id, 'Auto') }))
  expect(current(s)?.id).toBe('c')
  expect(s.unseen).toBe(0)
  useMeeting.setState(s)
  useMeeting.getState().move(-1)
  s = reduce(useMeeting.getState(), ev({ type: 'Brief', brief: brief('d', 'Auto') }))
  expect(current(s)?.id).toBe('b')
  expect(s.unseen).toBe(1)
  // Up once: `c` was seen before, `d` is still the one new brief above.
  useMeeting.setState(s)
  useMeeting.getState().move(1)
  expect(current(useMeeting.getState())?.id).toBe('c')
  expect(useMeeting.getState().unseen).toBe(1)
  useMeeting.getState().move(1)
  expect(current(useMeeting.getState())?.id).toBe('d')
  expect(useMeeting.getState().unseen).toBe(0)
})

test('a brief the user asked for jumps into view and clears the badge; its topic button goes on TopicRemoved{Viewed}', () => {
  let s = fresh()
  s = reduce(s, { type: 'TopicAdded', meeting: M, topic: { id: 't1', label: 'KEDA', at_ms: 0n, brief: null, cached: null, source: null } })
  for (const id of ['a', 'b']) s = reduce(s, ev({ type: 'Brief', brief: brief(id, 'Auto') }))
  useMeeting.setState(s)
  useMeeting.getState().move(-1)
  useMeeting.getState().markTapped('t1')
  s = reduce(useMeeting.getState(), ev({ type: 'Brief', brief: brief('c', 'Auto') }))
  expect(s.unseen).toBe(1)
  s = reduce(s, ev({ type: 'Brief', brief: brief('tap', 'Tap', 't1') }))
  expect(current(s)?.id).toBe('tap')
  expect(s.unseen).toBe(0)
  expect(s.tapped).toEqual([])
  expect(s.topics.map((t) => t.id)).toEqual(['t1'])
  s = reduce(s, { type: 'TopicRemoved', meeting: M, topic: 't1', why: 'Viewed' })
  expect(s.topics).toEqual([])
})

test('the cues end with the recording and Deleted empties the slice', () => {
  let s = fresh()
  s = reduce(s, { type: 'Listening', meeting: M, held: true })
  s = reduce(s, { type: 'ApprovalWaiting', meeting: M, waiting: true })
  expect([s.listening, s.approvalWaiting]).toEqual([true, true])
  s = reduce(s, { type: 'State', meeting: M, state: 'Done', degraded: ['MicNoAec'] })
  expect([s.listening, s.approvalWaiting, s.state, s.degraded]).toEqual([false, false, 'Done', ['MicNoAec']])
  s = reduce(s, { type: 'Deleted', meeting: M })
  expect(s.meeting).toBeNull()
})

test('PostProgress moves the state into Post with its pct; ReviewReady lands on Review and drops the bar', () => {
  let s = fresh()
  s = reduce(s, { type: 'State', meeting: M, state: { Post: 'Transcribing' }, degraded: [] })
  s = reduce(s, { type: 'PostProgress', meeting: M, stage: 'Transcribing', pct: 30 })
  expect([s.state, s.progress]).toEqual([{ Post: 'Transcribing' }, { stage: 'Transcribing', pct: 30 }])
  s = reduce(s, { type: 'PostProgress', meeting: M, stage: 'Diarizing', pct: 70 })
  expect(s.state).toEqual({ Post: 'Diarizing' })
  // A State event for the same Post stage keeps the bar; ReviewReady clears it.
  s = reduce(s, { type: 'State', meeting: M, state: { Post: 'Diarizing' }, degraded: [] })
  expect(s.progress?.pct).toBe(70)
  s = reduce(s, { type: 'PostProgress', meeting: 'other', stage: 'Aligning', pct: 95 })
  expect(s.progress?.pct).toBe(70)
  s = reduce(s, { type: 'ReviewReady', meeting: M })
  expect([s.state, s.progress]).toEqual(['Review', null])
})

const verdict = (batch_id: string, kind: 'skip' | 'brief' | 'model', outcome: OracleDecision['outcome'] = null): OracleDecision => ({
  batch_id,
  family: 'meeting.kind',
  mode: 'shadow',
  disposition: kind === 'skip' ? { kind } : kind === 'brief' ? { kind, brief: 'b1', title: 'SRE-2386' } : { kind, reason: 'low margin' },
  stages: [['kind: small talk', 0.79]],
  mentions: [],
  subject: null,
  tier2: null,
  top: [],
  latency_ms: 78n,
  tier: 1,
  outcome,
})

test('57 §3.4: OracleDeciding lights the chip; a verdict replaces it and an outcome replaces its row; counters fold', () => {
  let s = fresh()
  s = reduce(s, { type: 'OracleDeciding', meeting: M, batch_id: '00:14:32' })
  expect(s.deciding).toBe('00:14:32')
  s = reduce(s, { type: 'OracleDecision', meeting: M, decision: verdict('00:14:32', 'skip') })
  expect([s.deciding, s.oracle.length]).toEqual([null, 1])
  s = reduce(s, { type: 'OracleDecision', meeting: M, decision: verdict('00:15:01', 'model') })
  s = reduce(s, { type: 'OracleDecision', meeting: M, decision: verdict('00:15:30', 'brief') })
  // The outcome update keeps the row's place and count.
  s = reduce(s, { type: 'OracleDecision', meeting: M, decision: verdict('00:15:01', 'model', { agreed: false, published: 'HR-12' }) })
  expect(s.oracle.map((d) => d.batch_id)).toEqual(['00:14:32', '00:15:01', '00:15:30'])
  expect(s.oracle[1]?.outcome?.published).toBe('HR-12')
  s = reduce(s, { type: 'OracleDecision', meeting: M, decision: verdict('00:14:32', 'skip', { agreed: true, published: null }) })
  expect(oracleCounters(s.oracle)).toEqual({ skipped: 1, briefs: 1, escalated: 1, tier2: 0, agreed: 50 })
  expect(oracleSummary(s.oracle)).toBe('⚡ 1 skipped · 1 brief · 1 escalated · 0 tier 2 calls · agreed 50 %')
  // Another meeting's verdicts are ignored; the ring keeps the last ORACLE_KEEP.
  s = reduce(s, { type: 'OracleDecision', meeting: 'other', decision: verdict('00:16:00', 'skip') })
  expect(s.oracle.length).toBe(3)
  for (let i = 0; i < ORACLE_KEEP + 5; i++) s = reduce(s, { type: 'OracleDecision', meeting: M, decision: verdict(`01:00:${String(i).padStart(2, '0')}`, 'skip') })
  expect(s.oracle.length).toBe(ORACLE_KEEP)
  expect(oracleSummary([])).toBeNull()
})

const said = (track: 'Mic' | 'System', text: string, t0: number, final = true, meeting = M): MeetingEvent => ({
  type: 'Utterance',
  meeting,
  track,
  t0_ms: BigInt(t0),
  t1_ms: BigInt(t0 + 500),
  text,
  request: false,
  final,
})

test('02 §2: a partial replaces its track’s open row in place, a final commits it; other tracks and meetings are untouched', () => {
  let s = fresh()
  s = reduce(s, said('System', 'so the', 1000, false))
  s = reduce(s, said('Mic', 'right', 1200))
  s = reduce(s, said('System', 'so the DR', 1000, false))
  expect(s.lines.map((l) => [l.track, l.text, l.open])).toEqual([
    ['Mic', 'right', false],
    ['System', 'so the DR', true],
  ])
  s = reduce(s, said('System', 'so the DR sync', 1000))
  expect(s.lines.map((l) => [l.text, l.open])).toEqual([
    ['right', false],
    ['so the DR sync', false],
  ])
  s = reduce(s, said('Mic', 'elsewhere', 2000, true, 'other'))
  expect(s.lines.length).toBe(2)
  for (let i = 0; i < LINES_KEEP + 5; i++) s = reduce(s, said('Mic', `l${i}`, 3000 + i))
  expect(s.lines.length).toBe(LINES_KEEP)
  expect(s.lines[0]?.text).toBe('l5')
})
