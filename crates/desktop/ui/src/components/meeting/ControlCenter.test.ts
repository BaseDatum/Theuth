import { expect, test, vi } from 'vitest'
// The page embeds the chat pane, whose `nudges` store reads `localStorage` at import; no DOM in the runner.
vi.hoisted(() => {
  vi.stubGlobal('localStorage', { getItem: () => null, setItem: () => undefined })
})
import { STEPS, degradationText, fallbackText, gapText, lifecycle } from './ControlCenter'

test('no strip while live, before the recording ended, or for an unfinished start', () => {
  expect(lifecycle(undefined, null, null, true)).toBeNull()
  expect(lifecycle('Recording', null, null, false)).toBeNull()
  expect(lifecycle('Failed', null, 'no mic', false)).toBeNull()
  expect(lifecycle('Stopping', null, null, true)).toBeNull()
})

test('pipeline stages are busy, the store stage wins over the manifest', () => {
  expect(lifecycle({ Post: 'Diarizing' }, null, null, true)).toEqual({ current: 1, busy: true, failure: null, action: null })
  expect(lifecycle('Review', 'Aligning', null, true)).toEqual({ current: 2, busy: true, failure: null, action: null })
})

test('review waits on the user; a summary failure offers a retry', () => {
  expect(lifecycle('Review', null, null, true)).toEqual({ current: STEPS.indexOf('Review'), busy: false, failure: null, action: null })
  expect(lifecycle('Review', null, 'summary did not complete: its turn did not finish within 10 minutes', true)).toEqual({
    current: STEPS.indexOf('Review'),
    busy: false,
    failure: 'summary did not complete: its turn did not finish within 10 minutes',
    action: 'summarize',
  })
})

test('summarizing spins, done is the last step, failed offers the pipeline', () => {
  expect(lifecycle('Summarizing', null, null, true)).toEqual({ current: STEPS.indexOf('Summarizing'), busy: true, failure: null, action: null })
  expect(lifecycle('Done', null, null, true)).toEqual({ current: STEPS.length - 1, busy: false, failure: null, action: null })
  expect(lifecycle('Failed', null, 'align: no words', true)).toEqual({ current: null, busy: false, failure: 'align: no words', action: 'post_retry' })
})

test('173: degradationText names the track or permission', () => {
  expect(degradationText('MicNoAec')).toBe('MicNoAec')
  expect(degradationText({ SttDown: 'System' })).toBe('STT down (System)')
  expect(degradationText({ PermissionDenied: 'Microphone' })).toBe('Microphone permission denied')
  expect(degradationText({ PermissionDenied: 'AudioCapture' })).toBe('System Audio Recording permission denied')
})

test('173: gapText reads track, clock, length, reason and the retarget', () => {
  expect(gapText({ at_ms: 370_000n, dur_ms: 38_400n, track: 'system', reason: 'Silent' })).toBe('system 00:06:10 for 38 s (silent)')
  expect(gapText({ at_ms: 370_000n, dur_ms: 38_400n, track: 'system', reason: 'Stalled', retarget: 4242 })).toBe('system 00:06:10 for 38 s (stalled, tap moved to pid 4242)')
})

test('173 R3: fallbackText spans the clock, open-ended until a return probe', () => {
  expect(fallbackText({ reason: 'Silent', at_ms: 370_000n, until_ms: 450_000n })).toBe('00:06:10–00:07:30 (silent)')
  expect(fallbackText({ reason: 'Silent', at_ms: 370_000n, until_ms: null })).toBe('from 00:06:10 (silent)')
})
