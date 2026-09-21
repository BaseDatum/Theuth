import { expect, test } from 'vitest'
import type { OracleDecision } from '@/rpc/bindings/OracleDecision'
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

// 57 §3.4: the `⚡` row text and which stamps a batch card owns.
test('oracle rows read as the design shows them; carried stamps attach to the delivering card', () => {
  const base: OracleDecision = {
    batch_id: '00:14:32',
    family: 'meeting.topic',
    mode: 'active',
    disposition: { kind: 'skip' },
    top: [['topic', 0.12]],
    latency_ms: 78n,
    tier: 1,
    outcome: null,
  }
  expect(oracleRow(base)).toBe('⚡ topic 0.12 · skip · 78 ms')
  expect(
    oracleRow({
      ...base,
      family: 'meeting.brief',
      disposition: { kind: 'brief', brief: 'b1', title: 'SRE-2386' },
      top: [['topic', 0.7], ['SRE-2386', 0.91], ['none', 0.05]],
      latency_ms: 81n,
    }),
  ).toBe('⚡ brief SRE-2386 0.91 · published · 81 ms')
  expect(
    oracleRow({
      ...base,
      disposition: { kind: 'model', reason: 'low margin' },
      top: [['topic', 0.62], ['SRE-2386', 0.31], ['none', 0.55]],
      tier: 3,
    }),
  ).toBe('⚡ escalate · low margin 0.55 / 0.31 · tier 3 → model')
  expect(oracleRow({ ...base, tier: 2, outcome: { agreed: true, published: null } })).toBe('⚡ topic 0.12 · skip · tier 2 · 78 ms · ✓ agreed')
  expect(oracleRow({ ...base, mode: 'shadow', outcome: { agreed: false, published: 'HR-12' } })).toBe('⚡ topic 0.12 · skip · 78 ms · ✗ model published HR-12')

  const plain = parseMeetingLine('[meeting 00:14:32] live: none · dismissed: none\nthem: hi')
  expect(plain && batchStamps(plain)).toEqual(['00:14:32'])
  const carried = parseMeetingLine('[meeting 00:14:32] live: none · dismissed: none · earlier (oracle skipped): 00:13:10, 00:13:40\nthem: hi\noracle: topic 0.62 (escalated: low margin)')
  expect(carried && batchStamps(carried)).toEqual(['00:13:10', '00:13:40', '00:14:32'])
  const tap = parseMeetingLine('[meeting tapped "KEDA"]')
  expect(tap && batchStamps(tap)).toEqual([])
})
