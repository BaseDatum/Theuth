import { expect, test } from 'vitest'
import type { Enrolled } from "@/rpc/bindings/method.meeting.output"
import type { Segment } from "@/rpc/bindings/method.meeting.output"
import { alreadyEnrolled, enrollBlock, enrolledToast, rowBlock, secondsText, selectClick, speechMs } from './segment-select'

/** A segment of `n` words of 400 ms each, starting at `t0`. */
const seg = (id: number, track: Segment['track'], t0: number, n: number): Segment => ({
  id,
  track,
  speaker: track === 'Mic' ? 'me' : 's1',
  t0_ms: t0,
  t1_ms: t0 + n * 500,
  text: 'w '.repeat(n).trim(),
  words: Array.from({ length: n }, (_, k) => ({ text: 'w', t0_ms: t0 + k * 500, t1_ms: t0 + k * 500 + 400 })),
})

const segments = [seg(1, 'Mic', 0, 2), seg(2, 'System', 1000, 3), seg(3, 'System', 3000, 1), seg(4, 'Mic', 4000, 4), seg(5, 'System', 7000, 2)]
const order = segments.map((s) => s.id)

test('selectClick toggles on a plain click and fills the range on shift-click, either direction, keeping earlier picks', () => {
  let sel = selectClick(new Set(), order, null, 2, false)
  expect([...sel]).toEqual([2])
  sel = selectClick(sel, order, 2, 4, true)
  expect([...sel].sort()).toEqual([2, 3, 4])
  sel = selectClick(sel, order, 4, 1, true)
  expect([...sel].sort()).toEqual([1, 2, 3, 4])
  sel = selectClick(sel, order, 1, 3, false)
  expect([...sel].sort()).toEqual([1, 2, 4])
  // Shift without an anchor, or with an id that is no longer in the list, is a plain toggle.
  expect([...selectClick(new Set(), order, null, 5, true)]).toEqual([5])
  expect([...selectClick(new Set([1]), order, 99, 5, true)].sort()).toEqual([1, 5])
})

test('speechMs sums word spans, not segment spans, over the chosen ids only', () => {
  expect(speechMs(segments, [1])).toBe(800)
  expect(speechMs(segments, new Set([1, 4]))).toBe(2400)
  expect(speechMs(segments, [])).toBe(0)
  expect(speechMs(segments, [42])).toBe(0)
})

test('enrollBlock refuses both tracks, then under 1 s of speech; otherwise null', () => {
  expect(enrollBlock(segments, [1, 2])).toBe('pick segments from one track')
  expect(enrollBlock(segments, [3])).toBe('pick at least 1 s of speech')
  expect(enrollBlock(segments, [1])).toBe('pick at least 1 s of speech')
  expect(enrollBlock(segments, [2])).toBeNull()
  expect(enrollBlock(segments, [1, 4])).toBeNull()
  expect(enrollBlock(segments, [3, 5])).toBeNull()
})

test('rowBlock judges one segment by its own word spans and agrees with enrollBlock on that segment', () => {
  for (const s of segments) expect(rowBlock(s)).toBe(enrollBlock(segments, [s.id]))
  expect(rowBlock(seg(9, 'Mic', 0, 2))).toBe('pick at least 1 s of speech')
  expect(rowBlock(seg(9, 'Mic', 0, 3))).toBeNull()
})

const enrolled = (speech_ms: number, available_ms: number, samples: number): Enrolled => ({
  profile: { id: 'p1', name: 'Dana', entity: null, samples, created_at: '', updated_at: '' },
  speech_ms,
  available_ms,
  transcript: { meeting: 'm', speakers: [], segments: [], reviewed_at: null, summarized_at: null, diarized: true, note: null },
})

test('enrolledToast wording for New and Existing, with the cap note only when the engine used less than offered', () => {
  expect(enrolledToast(enrolled(12_000, 12_000, 1), { kind: 'New', name: 'Dana', entity: null, gated: false })).toBe('Enrolled Dana from 12 s of speech')
  expect(enrolledToast(enrolled(12_400, 12_400, 4), { kind: 'Existing', profile: 'p1' })).toBe('Added 12 s to Dana (4 samples)')
  expect(enrolledToast(enrolled(30_000, 74_000, 1), { kind: 'New', name: 'Dana', entity: null, gated: false })).toBe('Enrolled Dana from 30 s of speech (using 30 s of 74 s)')
  expect(secondsText(840)).toBe('0.8 s')
  expect(secondsText(23_400)).toBe('23 s')
})

test('alreadyEnrolled pulls the name out of the engine refusal and nothing else', () => {
  expect(alreadyEnrolled('a speaker named Dana Scully is already enrolled')).toBe('Dana Scully')
  expect(alreadyEnrolled('segments span both tracks')).toBeNull()
})
