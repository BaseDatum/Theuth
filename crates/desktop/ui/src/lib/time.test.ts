import { expect, test } from 'vitest'
import { fmtThought, formatAt, gapLabel, splitRestart, splitTime, withGaps } from './time'

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

test('splitTime takes only a leading <time …/> line off the text and reads its instant', () => {
  const stamp = '<time now="2026-09-28 14:03 -07:00 (Mon)" since_last="1h 12m"/>'
  const at = Date.parse('2026-09-28T14:03-07:00')
  expect(splitTime(`${stamp}\nhello`)).toEqual({ text: 'hello', at })
  expect(splitTime(`${stamp}\n<message from="x">y</message>`)).toEqual({ text: '<message from="x">y</message>', at })
  expect(splitTime(`  ${stamp}\nhello`)).toEqual({ text: 'hello', at })
  expect(splitTime(stamp)).toEqual({ text: '', at })
  // The first stamp of a session has no `since_last`; a malformed instant still strips, with no time.
  expect(splitTime('<time now="2026-10-03 00:05 +01:00 (Sat)"/>\nhi')).toEqual({ text: 'hi', at: Date.parse('2026-10-03T00:05+01:00') })
  expect(splitTime('<time now="garbage"/>\nhi')).toEqual({ text: 'hi' })
  expect(splitTime('hello')).toEqual({ text: 'hello' })
  expect(splitTime('<timer>x</timer>')).toEqual({ text: '<timer>x</timer>' })
  expect(splitTime('a\n<time now="x"/>\nb')).toEqual({ text: 'a\n<time now="x"/>\nb' })
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

test('fmtThought: nothing under a second, seconds, minutes, hours (91)', () => {
  expect(fmtThought(0)).toBeUndefined()
  expect(fmtThought(999)).toBeUndefined()
  expect(fmtThought(1000)).toBe('1s')
  expect(fmtThought(9400)).toBe('9s')
  expect(fmtThought(9600)).toBe('10s')
  expect(fmtThought(59_400)).toBe('59s')
  expect(fmtThought(59_600)).toBe('1m')
  expect(fmtThought(72_000)).toBe('1m 12s')
  expect(fmtThought(120_000)).toBe('2m')
  expect(fmtThought(3_600_000)).toBe('1h')
  expect(fmtThought(3_720_000)).toBe('1h 2m')
})

test('183 §12.4: splitRestart lifts the restart note (and the time stamp, either order) off a tool result', () => {
  const note = '<engine_restart at="11:42:07" after="4s" kind="takeover" from="mac" to="xeus" replayed="0" interrupted="1"/>'
  const stamp = '<time now="2026-11-08 11:42 +00:00 (Sun)"/>'
  const restart = { clock: '11:42:07', after: 4, cause: 'takeover', from: 'mac', to: 'xeus', replayed: 0, interrupted: 1, text: note }
  expect(splitRestart(`${note}\nERROR: interrupted`)).toEqual({ text: 'ERROR: interrupted', restart })
  expect(splitRestart(`${stamp}\n${note}\nok`)).toEqual({ text: 'ok', restart })
  expect(splitRestart(`${note}\n${stamp}\nok`)).toEqual({ text: 'ok', restart })
  expect(splitRestart(`${stamp}\nok`)).toEqual({ text: 'ok' })
  expect(splitRestart('plain <engine_restart at="x"/>')).toEqual({ text: 'plain <engine_restart at="x"/>' })
})
