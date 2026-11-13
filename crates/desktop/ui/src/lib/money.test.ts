import { expect, test } from 'vitest'
import type { CostBucket } from "@/rpc/bindings/method.agent.output"
import { costLines, costShort, formatMoney, moneyParts } from './money'

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
  expect(formatMoney(usd('123000000'))).toBe('$0.123')
  expect(formatMoney(usd('100000000'))).toBe('$0.100')
  expect(formatMoney(usd('0'))).toBe('$0.000')
  expect(formatMoney(usd('1234499999'))).toBe('$1.234')
  expect(formatMoney(usd('1234500000'))).toBe('$1.235')
  expect(formatMoney(usd('99999999999999999999'))).toBe('$100000000000.000')
})

test('moneyParts prints formatMoney digit for digit through Intl', () => {
  for (const nanos of ['123000000', '100000000', '0', '1234499999', '1234500000', '1004500000', '99999999999999999999']) {
    const { prefix, value } = moneyParts(usd(nanos))
    expect(prefix + value.toLocaleString('en', { minimumFractionDigits: 3, maximumFractionDigits: 3, useGrouping: false })).toBe(formatMoney(usd(nanos)))
  }
  expect(moneyParts({ currency: 'XYZ', nanos: '5000000' }).prefix).toBe('XYZ ')
})

test('costLines follows 51 §6.2 and never merges returned with estimate', () => {
  expect(costLines(bucket({ calls: 2 }))).toEqual({ primary: 'Returned cost —' })
  expect(costLines(bucket({ dispatched: 3, returned: [usd('123000000')] }))).toEqual({ primary: 'Returned cost $0.123' })
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
    primary: 'Returned cost $0.100 · 2 unavailable',
    secondary: 'Estimated ~$0.023',
  })
  expect(costLines(bucket({ dispatched: 2, unavailable: 2 }))).toEqual({ primary: 'Returned cost — · 2 unavailable' })
  expect(costLines(bucket({ dispatched: 2, returned: [usd('1000000000'), { currency: 'EUR', nanos: '500000000' }] })).primary).toBe(
    'Returned cost $1.000 + €0.500',
  )
})

test('costShort shows returned money and the tilde-marked estimate together; a zero meter never hides the estimate', () => {
  const est = [usd('23000000')]
  expect(costShort(bucket({ dispatched: 2, returned: [usd('100000000')], estimated_for_unavailable: est }))).toBe('$0.100 + ~$0.023')
  expect(costShort(bucket({ dispatched: 2, estimated_for_unavailable: est }))).toBe('~$0.023')
  expect(costShort(bucket({ dispatched: 2, returned: [usd('100000000')] }))).toBe('$0.100')
  // omp child connected (known-zero meter rolled up) over an estimate-only parent.
  expect(costShort(bucket({ dispatched: 2, unavailable: 2, returned: [usd('0')], estimated_for_unavailable: est }))).toBe('~$0.023')
  expect(costShort(bucket({ dispatched: 1, returned: [usd('0')] }))).toBe('$0.000')
  expect(costShort(bucket({ dispatched: 2, unavailable: 2, returned: [usd('0')] }))).toBe('—')
  expect(costShort(bucket({ dispatched: 2, unavailable: 2 }))).toBe('—')
  expect(costShort(bucket({}))).toBe('—')
  const eur = (nanos: string) => ({ currency: 'EUR', nanos })
  expect(
    costShort(bucket({ dispatched: 4, unavailable: 1, returned: [eur('500000000'), usd('0')], estimated_for_unavailable: [eur('10000000'), usd('23000000')] })),
  ).toBe('€0.500 + ~€0.010 + ~$0.023')
  expect(costShort(bucket({ dispatched: 2, returned: [eur('500000000'), usd('1000000000')], estimated_for_unavailable: est }))).toBe('€0.500 + $1.000 + ~$0.023')
})
