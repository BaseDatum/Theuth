import { expect, test } from 'vitest'
import { band, meterSource } from './ContextMeter'

test('bands at 75 / 90 / 95 % (75 §5); no limit is normal', () => {
  expect([null, 0, 74, 75, 89, 90, 95, 96, 120].map(band)).toEqual([
    'normal',
    'normal',
    'normal',
    'warning',
    'warning',
    'danger',
    'danger',
    'critical',
    'critical',
  ])
})

test('141 R3 T14: a watch-only omp child reads its own contextTokens/contextWindow; without them the session figures stand', () => {
  const omp = { detail: { contextTokens: 9037, contextWindow: 1_000_000 } }
  expect(meterSource(omp, 0, true, undefined)).toEqual({ used: 9037, estimated: false, limit: 1_000_000 })
  // Tail-only (no window from feed 1): the harness's tokens over the stored size.
  expect(meterSource({ detail: { contextTokens: 717 } }, 0, true, 200_000)).toEqual({ used: 717, estimated: false, limit: 200_000 })
  expect(meterSource({ detail: null }, 1234, true, 200_000)).toEqual({ used: 1234, estimated: true, limit: 200_000 })
  expect(meterSource(null, undefined, undefined, undefined)).toEqual({ used: undefined, estimated: false, limit: undefined })
})
