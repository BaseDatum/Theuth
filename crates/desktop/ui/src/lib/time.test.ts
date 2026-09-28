import { expect, test } from 'vitest'
import { formatAt, gapLabel, stripTime, withGaps } from './time'

// Local wall-clock instants, so the expectations hold in any zone.
const at = (y: number, mo: number, d: number, h: number, mi: number) => new Date(y, mo - 1, d, h, mi).getTime()
const now = at(2026, 9, 28, 16, 0) // Monday

test('formatAt: today, this week, older', () => {
  expect(formatAt(at(2026, 9, 28, 14, 3), now)).toBe('14:03')
  expect(formatAt(at(2026, 9, 28, 0, 5), now)).toBe('00:05')
  expect(formatAt(at(2026, 9, 27, 23, 59), now)).toBe('Sun 23:59')
  expect(formatAt(at(2026, 9, 22, 9, 0), now)).toBe('Tue 09:00')
  expect(formatAt(at(2026, 9, 21, 9, 0), now)).toBe('21 Sep 09:00')
  expect(formatAt(at(2025, 1, 2, 9, 0), now)).toBe('2 Jan 09:00')
})

test('gapLabel: only at an hour or more, with the later time', () => {
  const t0 = at(2026, 9, 28, 14, 3)
  expect(gapLabel(t0, t0 + 59 * 60_000, now)).toBeNull()
  expect(gapLabel(t0, t0 + 60 * 60_000, now)).toBe('1 h later · 15:03')
  expect(gapLabel(t0, t0 + 72 * 60_000, now)).toBe('1 h 12 min later · 15:15')
  expect(gapLabel(at(2026, 9, 26, 10, 0), at(2026, 9, 27, 12, 30), now)).toBe('26 h 30 min later · Sun 12:30')
  expect(gapLabel(undefined, t0, now)).toBeNull()
  expect(gapLabel(t0, undefined, now)).toBeNull()
})

test('stripTime removes only a leading <time …/> line', () => {
  const stamp = '<time now="2026-09-28 14:03 -07:00 (Mon)" since_last="1h 12m"/>'
  expect(stripTime(`${stamp}\nhello`)).toBe('hello')
  expect(stripTime(`${stamp}\n<message from="x">y</message>`)).toBe('<message from="x">y</message>')
  expect(stripTime(`  ${stamp}\nhello`)).toBe('hello')
  expect(stripTime(stamp)).toBe('')
  expect(stripTime('hello')).toBe('hello')
  expect(stripTime('<timer>x</timer>')).toBe('<timer>x</timer>')
  expect(stripTime('a\n<time now="x"/>\nb')).toBe('a\n<time now="x"/>\nb')
})

test('withGaps: a divider before each row an hour past the last timed row; untimed rows are skipped over', () => {
  const t0 = at(2026, 9, 28, 10, 0)
  const rows = [
    { key: 'a', at: t0 },
    { key: 'b', at: t0 + 30 * 60_000 },
    { key: 'c' as string, at: undefined as number | undefined },
    { key: 'd', at: t0 + 100 * 60_000 },
    { key: 'e', at: t0 + 101 * 60_000 },
  ]
  expect(withGaps(rows, (r) => r.at, now).map((r) => ('gap' in r ? `[${r.gap}]` : r.key))).toEqual([
    'a',
    'b',
    'c',
    '[1 h 10 min later · 11:40]',
    'd',
    'e',
  ])
  expect(withGaps([], () => undefined, now)).toEqual([])
})
