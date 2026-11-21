import { describe, expect, test } from 'vitest'
import type { UsageSample } from '@/rpc/bindings/method.agent.output'
import { pace } from './pace'

const H = 3_600_000
// A 10-hour window from 00:00 to 10:00 (ms offsets), observed at `now`.
const WINDOW_SECS = 36_000
const RESET = 10 * H
const s = (hours: number, used: number): UsageSample => ({ at_ms: hours * H, used_fraction: used })
const win = (used: number, history: UsageSample[] = []) => ({ used_fraction: used, resets_at_ms: RESET, window_secs: WINDOW_SECS, history })

describe('pace', () => {
  test('no reset or no window length: no chart', () => {
    expect(pace({ used_fraction: 0.4, resets_at_ms: null, window_secs: 18_000 }, 0)).toBeNull()
    expect(pace({ used_fraction: 0.4, resets_at_ms: RESET, window_secs: null }, 0)).toBeNull()
    expect(pace({ used_fraction: 0.4, resets_at_ms: RESET, window_secs: 0 }, 0)).toBeNull()
  })

  test('fallback: straight line from the start through now', () => {
    const p = pace(win(0.3), 6 * H)!
    expect(p.mode).toBe('fallback')
    expect(p.start).toBe(0)
    expect(p.elapsed).toBeCloseTo(0.6)
    expect(p.projected).toBeCloseTo(0.5)
    expect(p.hitsAt).toBeNull()
    expect(p.line).toEqual([
      { t: 0, v: 0 },
      { t: 6 * H, v: 0.3 },
    ])
  })

  test('fallback over pace: crosses 100 % where the line does', () => {
    const p = pace(win(0.5), 2.5 * H)!
    expect(p.projected).toBeCloseTo(2)
    expect(p.hitsAt).toBeCloseTo(5 * H, -2)
  })

  test('elapsed ≈ 0: too early to project', () => {
    const p = pace(win(0.01), 0.1 * H)!
    expect(p.elapsed).toBeCloseTo(0.01)
    expect(p.projected).toBeNull()
    expect(p.hitsAt).toBeNull()
  })

  test('now outside the window clamps elapsed and the usage line', () => {
    const before = pace(win(0), -H)!
    expect(before.elapsed).toBe(0)
    expect(before.line.at(-1)).toEqual({ t: 0, v: 0 })
    const after = pace(win(0.8), 11 * H)!
    expect(after.elapsed).toBe(1)
    expect(after.projected).toBeCloseTo(0.8)
  })

  test('used over 100 %: projected stays above, no future crossing', () => {
    const p = pace(win(1.2), 5 * H)!
    expect(p.projected).toBeCloseTo(2.4)
    expect(p.hitsAt).toBeNull()
  })

  test('fewer than 3 samples, or too short a span: fallback', () => {
    expect(pace(win(0.3, [s(1, 0.1), s(6, 0.3)]), 6 * H)!.mode).toBe('fallback')
    // three readings within 0.5 h = 5 % of the window
    expect(pace(win(0.3, [s(5.5, 0.2), s(5.8, 0.25), s(6, 0.3)]), 6 * H)!.mode).toBe('fallback')
  })

  test('history: recent rate over the last 20 % of the window', () => {
    // 0.05 per hour until 4 h, then 0.1 per hour; last 20 % = from 4 h on (now 6 h).
    const history = [s(0, 0), s(2, 0.1), s(4, 0.2), s(5, 0.3), s(6, 0.4)]
    const p = pace(win(0.4, history), 6 * H)!
    expect(p.mode).toBe('history')
    expect(p.projected).toBeCloseTo(0.8)
    expect(p.line).toEqual(history.map((x) => ({ t: x.at_ms, v: x.used_fraction })))
  })

  test('history: a burst in the last 20 % shows up even though the average is low', () => {
    const history = [s(0, 0), s(2, 0.02), s(4, 0.05), s(5, 0.05), s(6, 0.35)]
    const p = pace(win(0.35, history), 6 * H)!
    // fallback would say 0.35 / 0.6 ≈ 0.58; the recent rate is 0.15 per hour → 0.35 + 0.6
    expect(p.projected).toBeCloseTo(0.95)
    expect(pace(win(0.35), 6 * H)!.projected).toBeCloseTo(0.583, 2)
    const hot = pace(win(0.6, [s(0, 0), s(2, 0.02), s(4, 0.05), s(5, 0.1), s(6, 0.6)]), 6 * H)!
    // 0.05 → 0.6 over the last 2 h = 0.275 per hour
    expect(hot.projected).toBeCloseTo(0.6 + 0.275 * 4)
    expect(hot.hitsAt).toBeCloseTo(6 * H + (0.4 / 0.275) * H, -2)
  })

  test('history: fewer than 2 samples in the recent span uses the last two', () => {
    const history = [s(0, 0), s(1, 0.1), s(3, 0.3)]
    const p = pace(win(0.3, history), 6 * H)!
    expect(p.mode).toBe('history')
    expect(p.projected).toBeCloseTo(0.3 + 0.1 * 4)
    // the line runs on flat from the newest reading to now
    expect(p.line.at(-1)).toEqual({ t: 6 * H, v: 0.3 })
  })

  test('history: a dip never projects negative use; readings from before the window are ignored', () => {
    const p = pace(win(0.39, [s(0, 0), s(3, 0.2), s(5, 0.4), s(6, 0.39)]), 6 * H)!
    expect(p.projected).toBeCloseTo(0.39)
    const stale = pace(win(0.3, [{ at_ms: -2 * H, used_fraction: 0.9 }, s(1, 0.1), s(6, 0.3)]), 6 * H)!
    expect(stale.mode).toBe('fallback')
  })
})
