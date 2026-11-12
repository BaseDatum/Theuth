import { expect, test } from 'vitest'
import type { Detection } from "@/rpc/bindings/method.meeting.output"
import { EMPTY, LAWFUL, detectedStart, reduce } from './detection'

const zoom: Detection = {
  app: { pid: 4242, name: 'Zoom', bundle_id: 'us.zoom.xos', active: true },
  app_id: 'us.zoom.xos',
  since: '2026-10-10T10:00:00Z',
  also: [],
  event: null,
  project: null,
  brief: null,
}

test('R3: one sentence, verbatim', () => {
  expect(LAWFUL).toBe('By starting the recording you acknowledge the recording is lawful.')
})

test('§5.4: a one-tap start sends the detected pid, consent: true and source Detected; every default is the engine\u2019s', () => {
  expect(detectedStart(zoom)).toEqual({
    process: 4242,
    title: null,
    context: null,
    consent: true,
    session_model: null,
    source: { kind: 'detected', app_id: 'us.zoom.xos' },
  })
  // W2/W3 fill `event` and `project`; when present they ride along as §5.4 lists.
  const ev = { id: 'e1', uid: 'u', title: 'Weekly sync', start: '2026-10-10T10:00:00Z', end: '2026-10-10T10:30:00Z', join_url: null, join_host: null, organizer: null, attendees: [], description: null }
  expect(detectedStart({ ...zoom, event: { event: ev, score: 0.9, alternatives: [] }, project: 'theuth' })).toMatchObject({ title: 'Weekly sync', event: 'e1', project: 'theuth' })
})

test('the banner follows the bus: Detected shows, DetectionCleared for that app hides, another app\u2019s clear is ignored, Started ends the prompt', () => {
  let s = reduce(EMPTY, { type: 'Detected', ...zoom })
  expect(s.detection?.app_id).toBe('us.zoom.xos')
  expect(reduce(s, { type: 'DetectionCleared', app_id: 'com.other', reason: 'NotNow' }).detection).not.toBeNull()
  expect(reduce(s, { type: 'DetectionCleared', app_id: 'us.zoom.xos', reason: 'Never' }).detection).toBeNull()
  s = reduce(s, { type: 'Started', meeting: 'm1', session: 's1', process: zoom.app, at: '2026-10-10T10:01:00Z' })
  expect(s.detection).toBeNull()
})

test('a clear for a prompt the bus never showed (the banner still on the RPC answer) marks the state live and empty', () => {
  const s = reduce(EMPTY, { type: 'DetectionCleared', app_id: zoom.app_id, reason: 'NotNow' })
  expect(s).toEqual({ ...EMPTY, live: true })
})

test('AppReleased asks Stop/Keep until Keep, or until that meeting leaves Recording', () => {
  const released = { type: 'AppReleased' as const, meeting: 'm1', app: zoom.app, released_for_s: 30 }
  const s = reduce(EMPTY, released)
  expect(s.released?.released_for_s).toBe(30)
  expect(reduce(s, { type: 'Keep' }).released).toBeNull()
  expect(reduce(s, { type: 'State', meeting: 'm2', state: 'Paused', degraded: [] }).released).not.toBeNull()
  expect(reduce(s, { type: 'State', meeting: 'm1', state: 'Recording', degraded: [] }).released).not.toBeNull()
  expect(reduce(s, { type: 'State', meeting: 'm1', state: { Post: 'Transcribing' }, degraded: [] }).released).toBeNull()
})
