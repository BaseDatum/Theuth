// 156 D7: the seed is what the page reads; this pins its shape (it type-checks against the bindings) and the facts
// the brief asked for: ~30 multi-word segments on both tracks under three labels, 4 briefs, one enrolled profile.
import { expect, test, vi } from 'vitest'
// `store/sessions` reads `localStorage` at import; no DOM in the runner.
vi.hoisted(() => {
  vi.stubGlobal('localStorage', { getItem: () => null, setItem: () => undefined })
})
import { FIXTURE_ID, seed } from './meeting-page-fixture'

test('the seed has both tracks, three labels, multi-word timed segments, four briefs and one profile; the state follows the mode', () => {
  const s = seed('review')
  expect(s.manifest.id).toBe(FIXTURE_ID)
  expect(s.manifest.state).toBe('Review')
  expect(s.manifest.ended_at).not.toBeNull()
  expect(s.transcript.segments.length).toBeGreaterThanOrEqual(28)
  expect(new Set(s.transcript.segments.map((x) => x.track))).toEqual(new Set(['Mic', 'System']))
  expect(new Set(s.transcript.segments.map((x) => x.speaker))).toEqual(new Set(['me', 's1', 's2']))
  expect(s.transcript.speakers.map((x) => x.id)).toEqual(['me', 's1', 's2'])
  for (const seg of s.transcript.segments) {
    expect(seg.words.length).toBeGreaterThan(3)
    expect(seg.words.map((w) => w.text).join(' ')).toBe(seg.text)
    expect(seg.t1_ms > seg.t0_ms).toBe(true)
  }
  // Sorted by start across both tracks, as the engine writes it.
  expect(s.transcript.segments.map((x) => x.t0_ms)).toEqual([...s.transcript.segments.map((x) => x.t0_ms)].sort((a, b) => Number(a - b)))
  expect(s.briefs).toHaveLength(4)
  expect(s.briefs.every((b) => b.topic !== null && b.bullets.length >= 2)).toBe(true)
  expect(s.speakers).toHaveLength(1)
  expect(s.live).toHaveLength(s.transcript.segments.length)
  expect(s.transcript.reviewed_at).toBeNull()

  expect(seed('live').manifest.state).toBe('Recording')
  expect(seed('live').manifest.ended_at).toBeNull()
  expect(seed('done').manifest.state).toBe('Done')
  expect(seed('done').transcript.reviewed_at).not.toBeNull()
})
