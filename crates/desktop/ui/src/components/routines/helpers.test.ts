import { expect, test } from 'vitest'
import { cadence, cronWords, movedSettingsTab, resultKind, sortRoutines } from './helpers'

test('cadence in words', () => {
  expect(cadence({ when: 'every 5m', describe: 'every 5m' })).toBe('every 5m')
  expect(cadence({ when: 'cron 0 9 * * *', describe: 'x' })).toBe('every day 09:00')
  expect(cadence({ when: 'cron 30 9 * * 1-5', describe: 'x' })).toBe('weekdays 09:30')
  expect(cadence({ when: 'cron 0 18 * * 5', describe: 'x' })).toBe('every Friday 18:00')
  expect(cadence({ when: 'cron */15 * * * *', describe: 'x' })).toBe('every 15 min')
  expect(cadence({ when: 'cron 5 * * * *', describe: 'x' })).toBe('hourly at :05')
  expect(cadence({ when: 'cron 0 9 1 * *', describe: 'x' })).toBe('monthly on day 1 09:00')
  // Unhandled shapes fall back to the engine's croner text.
  expect(cronWords('0 9 * 6 *')).toBeNull()
  expect(cadence({ when: 'cron 0 9 * 6 *', describe: 'At 09:00 in June' })).toBe('At 09:00 in June')
  expect(cadence({ when: 'at 2026-09-16T15:00', describe: 'x' })).toMatch(/^once at .*15:00$/)
  expect(cadence({ when: '—', describe: '—' })).toBe('—')
})

test('list order: broken, then enabled by next, then paused; name breaks ties', () => {
  const row = (name: string, o: Partial<{ enabled: boolean; error: string | null; next: string | null }> = {}) => ({
    name,
    enabled: true,
    error: null,
    next: null,
    ...o,
  })
  const sorted = sortRoutines([
    row('zeta-paused', { enabled: false }),
    row('later', { next: '2030-01-01T10:00:00Z' }),
    row('never-next'),
    row('alpha-paused', { enabled: false }),
    row('soon', { next: '2030-01-01T09:00:00Z' }),
    row('broken', { error: 'cron: bad', enabled: false }),
  ]).map((s) => s.name)
  expect(sorted).toEqual(['broken', 'soon', 'later', 'never-next', 'alpha-paused', 'zeta-paused'])
})

test('last result chip', () => {
  expect(resultKind(null)).toBeNull()
  expect(resultKind('end_turn')).toBe('ok')
  expect(resultKind('done (model): merged')).toBe('ok')
  expect(resultKind('max_runs 200 reached')).toBe('ok')
  expect(resultKind('skipped: busy')).toBe('skipped')
  for (const r of ['error: boom', 'timeout', 'cancelled', 'lost', 'until broken: exit 2']) expect(resultKind(r)).toBe('failed')
})

test('/settings/schedules redirects to /routines; other tabs stay', () => {
  expect(movedSettingsTab('schedules')).toBe('/routines')
  expect(movedSettingsTab('general')).toBeNull()
  expect(movedSettingsTab('nonsense')).toBeNull()
})
