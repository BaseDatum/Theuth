import { expect, test } from 'vitest'
import type { Brief } from '@/rpc/bindings/Brief'
import type { BriefSource } from '@/rpc/bindings/BriefSource'
import type { MeetingEvent } from '@/rpc/bindings/MeetingEvent'
import { type MeetingSlice, current, reduce, useMeeting } from './meeting'

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
  s = reduce(s, { type: 'TopicAdded', meeting: M, topic: { id: 't1', label: 'KEDA', at_ms: 0n, brief: null, cached: null } })
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
