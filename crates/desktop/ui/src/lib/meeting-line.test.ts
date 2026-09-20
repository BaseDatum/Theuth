import { expect, test } from 'vitest'
import { parseMeetingLine } from './meeting-line'

// The exact strings `crates/meeting/src/session.rs` writes (02 §3).
test('recognises every daemon line shape and nothing else', () => {
  expect(parseMeetingLine('[meeting 00:14:32] live: SRE-2386 · dismissed: none\nthem: so the DR sync\nme: right')).toEqual({
    kind: 'batch',
    label: 'transcript 00:14:32',
    body: 'live: SRE-2386 · dismissed: none\nthem: so the DR sync\nme: right',
  })
  expect(parseMeetingLine('[meeting 00:00:00] started. context: none')).toEqual({ kind: 'started', label: 'meeting started', body: 'context: none' })
  expect(parseMeetingLine('[meeting request, typed] SQS')).toEqual({ kind: 'request', label: 'request · typed', body: 'SQS' })
  expect(parseMeetingLine('[meeting request, spoken] what is the cutover date')?.label).toBe('request · spoken')
  expect(parseMeetingLine('[meeting tapped "KEDA"]')).toEqual({ kind: 'tapped', label: 'tapped “KEDA”', body: '' })
  expect(parseMeetingLine('[meeting: user dismissed "KEDA" — not useful]')).toEqual({ kind: 'dismissed', label: 'dismissed “KEDA”', body: '' })
  expect(parseMeetingLine('[meeting deeper on "AWS EFS" — parent: 01a0be9e-0000-7000-8000-000000000000] - a\n- b')).toEqual({
    kind: 'deeper',
    label: 'deeper on “AWS EFS”',
    body: '- a\n- b',
  })
  expect(parseMeetingLine('[meeting: no brief was produced for the request "SQS" — the user sees only briefs, never text. Call `meeting_brief` now.]')).toEqual({
    kind: 'nudge',
    label: 'reminder: brief it',
    body: 'the request "SQS"',
  })
  expect(parseMeetingLine('what is [meeting 00:00:00]?')).toBeNull()
  expect(parseMeetingLine('[meeting] nonsense')).toBeNull()
})
