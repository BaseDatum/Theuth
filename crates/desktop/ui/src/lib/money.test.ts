import { expect, test } from 'vitest'
import type { CostBucket } from '@/rpc/bindings/CostBucket'
import { costLines, formatMoney } from './money'

const usd = (nanos: string) => ({ currency: 'USD', nanos })
const bucket = (o: Partial<CostBucket>): CostBucket => ({
  returned: [],
  estimated_for_unavailable: [],
  calls: 0,
  dispatched: 0,
  unavailable: 0,
  unresolved: 0,
  tokens: { input: 0, output: 0, cache_read: 0, cache_write: 0, unknown_calls: 0 },
  ...o,
})

test('formatMoney rounds nanos half up to milli-units without floats', () => {
  expect(formatMoney(usd('123000000'))).toBe('USD 0.123')
  expect(formatMoney(usd('100000000'))).toBe('USD 0.100')
  expect(formatMoney(usd('0'))).toBe('USD 0.000')
  expect(formatMoney(usd('1234499999'))).toBe('USD 1.234')
  expect(formatMoney(usd('1234500000'))).toBe('USD 1.235')
  expect(formatMoney(usd('99999999999999999999'))).toBe('USD 100000000000.000')
})

test('costLines follows 51 §6.2 and never merges returned with estimate', () => {
  expect(costLines(bucket({ calls: 2 }))).toEqual({ primary: 'Returned cost —' })
  expect(costLines(bucket({ dispatched: 3, returned: [usd('123000000')] }))).toEqual({ primary: 'Returned cost USD 0.123' })
  expect(
    costLines(
      bucket({
        dispatched: 5,
        unavailable: 2,
        returned: [usd('100000000')],
        estimated_for_unavailable: [usd('23000000')],
      }),
    ),
  ).toEqual({
    primary: 'Returned cost USD 0.100 · 2 unavailable',
    secondary: 'Estimate for unavailable calls ~USD 0.023',
  })
  expect(costLines(bucket({ dispatched: 2, unavailable: 2 }))).toEqual({ primary: 'Returned cost — · 2 unavailable' })
  expect(costLines(bucket({ dispatched: 2, returned: [usd('1000000000'), { currency: 'EUR', nanos: '500000000' }] })).primary).toBe(
    'Returned cost USD 1.000 + EUR 0.500',
  )
})
