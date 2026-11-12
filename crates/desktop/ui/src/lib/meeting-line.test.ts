import { expect, test } from 'vitest'
import type { OracleDecision } from "@/rpc/bindings/event.meeting_2eevents.output"
import { batchStamps, oracleRow, parseMeetingLine } from './meeting-line'

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
  expect(parseMeetingLine('[meeting summary] The meeting has ended and the transcript was reviewed. Use `meeting_read`…')).toEqual({
    kind: 'summary',
    label: 'summary requested',
    body: 'The meeting has ended and the transcript was reviewed. Use `meeting_read`…',
  })
  expect(parseMeetingLine('[meeting: no brief was produced for the request "SQS" — the user sees only briefs, never text. Call `meeting_brief` now.]')).toEqual({
    kind: 'nudge',
    label: 'reminder: brief it',
    body: 'the request "SQS"',
  })
  expect(parseMeetingLine('what is [meeting 00:00:00]?')).toBeNull()
  expect(parseMeetingLine('[meeting] nonsense')).toBeNull()
})

// 57 §3.5: the oracle row text (stages in order, empty ones omitted) and which stamps a batch card owns.
test('oracle rows read as the design shows them; carried stamps attach to the delivering card', () => {
  const base: OracleDecision = {
    batch_id: '00:14:32',
    family: 'meeting.kind',
    mode: 'active',
    disposition: { kind: 'skip' },
    stages: [['kind: small talk', 0.79]],
    mentions: [],
    subject: null,
    tier2: null,
    top: [],
    latency_ms: 78,
    tier: 1,
    outcome: null,
  }
  expect(oracleRow(base)).toBe('kind: small talk 0.79 → skip · 78 ms')
  expect(
    oracleRow({
      ...base,
      family: 'meeting.brief',
      disposition: { kind: 'brief', brief: 'b1', title: 'SRE-2386' },
      stages: [['kind: subject', 0.88], ['brief: SRE-2386', 0.91]],
      mentions: ['SRE-2386'],
      top: [['SRE-2386', 0.91], ['none', 0.05]],
      latency_ms: 81,
    }),
  ).toBe('kind: subject 0.88 · mentions: SRE-2386 · brief: SRE-2386 0.91 → published SRE-2386 · 81 ms')
  expect(
    oracleRow({
      ...base,
      family: 'meeting.subject',
      disposition: { kind: 'model', reason: 'subject "Planet Nine", no cached brief' },
      stages: [['kind: subject', 0.91], ['brief: none', 0.57]],
      mentions: ['Kuiper belt', 'Constantin'],
      subject: 'Planet Nine',
      tier2: '"Planet Nine" → no cached brief',
      top: [['DR', 0.31], ['none', 0.57]],
      latency_ms: 1442,
      tier: 3,
    }),
  ).toBe('kind: subject 0.91 · mentions: Kuiper belt, Constantin · brief: none 0.57 · tier 2: "Planet Nine" → no cached brief → model · 1442 ms')
  expect(oracleRow({ ...base, tier: 2, tier2: 'nothing to look up', outcome: { agreed: true, published: null } })).toBe(
    'kind: small talk 0.79 · tier 2: nothing to look up → skip · 78 ms · ✓ agreed',
  )
  expect(oracleRow({ ...base, mode: 'shadow', outcome: { agreed: false, published: 'HR-12' } })).toBe('kind: small talk 0.79 → skip · 78 ms · ✗ model published HR-12')

  const plain = parseMeetingLine('[meeting 00:14:32] live: none · dismissed: none\nthem: hi')
  expect(plain && batchStamps(plain)).toEqual(['00:14:32'])
  const carried = parseMeetingLine('[meeting 00:14:32] live: none · dismissed: none · earlier (oracle skipped): 00:13:10, 00:13:40\nthem: hi\noracle: topic 0.62 (escalated: low margin)')
  expect(carried && batchStamps(carried)).toEqual(['00:13:10', '00:13:40', '00:14:32'])
  const tap = parseMeetingLine('[meeting tapped "KEDA"]')
  expect(tap && batchStamps(tap)).toEqual([])
})
