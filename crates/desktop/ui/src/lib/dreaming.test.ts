import { describe, expect, test } from 'vitest'
import { FIXTURES, NIGHT, QUIET_NIGHT, SETTINGS, UNDONE_NIGHT } from '@/dev/dreaming-fixture'
import { countsLine, formatDigest, isPaused, nightLine, parseDigest, pauseUntil, reduceSettings, resolvedDefault, undoLine, usd } from './dreaming'

describe('formatting', () => {
  test('counts line omits zeros, pluralises, and says so when nothing happened', () => {
    expect(countsLine(NIGHT.counts)).toBe('212 retitled, 14 merged, 3 superseded, 31 archived, 15 verified, 9 cases, 1 skill edit kept, 2 rejected, 1 proposal')
    expect(countsLine({ ...NIGHT.counts, edits_accepted: 2n, cases: 1n })).toContain('1 case, 2 skill edits kept')
    expect(countsLine(QUIET_NIGHT.counts)).toBe('nothing to tidy')
  })

  test('night line per fixture state: none, a night, an undone night', () => {
    expect(nightLine(FIXTURES.empty.status.last_night)).toBe('no nights yet')
    expect(nightLine(FIXTURES.night.status.last_night)).toBe(`last night: ${countsLine(NIGHT.counts)}, $4.100`)
    expect(nightLine(FIXTURES.paused.status.last_night)).toBe(`last night (undone): ${countsLine(UNDONE_NIGHT.counts)}, $0.930`)
  })

  test('usd goes through the money formatter (half-up at three decimals)', () => {
    expect(usd(1)).toBe('$1.000')
    expect(usd(0.0005)).toBe('$0.001')
    expect(usd(4.1)).toBe('$4.100')
  })

  test('undo toast line', () => {
    expect(undoLine({ night: 'n', group: null, knowledge_restored: ['a', 'b'], skills_restored: ['s'], edits_undone: [], errors: [] })).toBe('2 knowledge, 1 skill restored')
    expect(undoLine({ night: 'n', group: null, knowledge_restored: [], skills_restored: [], edits_undone: [], errors: ['boom', 'x'] })).toBe('nothing to restore · 2 errors: boom')
  })
})

describe('digest', () => {
  test('round-trips and tolerates long day names and junk', () => {
    expect(parseDigest('Mon 08:00')).toEqual({ day: 'Mon', time: '08:00' })
    expect(parseDigest('sunday 19:30')).toEqual({ day: 'Sun', time: '19:30' })
    expect(parseDigest('')).toEqual({ day: 'Mon', time: '08:00' })
    expect(formatDigest('Fri', '07:15')).toBe('Fri 07:15')
  })
})

describe('reduceSettings', () => {
  test('budget: any non-negative amount, no-op and negatives return null', () => {
    expect(reduceSettings(SETTINGS, { kind: 'budget', usd: 2.5 })?.budget_usd).toBe(2.5)
    expect(reduceSettings(SETTINGS, { kind: 'budget', usd: 0 })?.budget_usd).toBe(0)
    expect(reduceSettings(SETTINGS, { kind: 'budget', usd: 1 })).toBeNull()
    expect(reduceSettings(SETTINGS, { kind: 'budget', usd: -1 })).toBeNull()
    expect(reduceSettings(SETTINGS, { kind: 'budget', usd: Number.NaN })).toBeNull()
  })

  test('window edits one end, rejects a partial time, never aliases the tuple', () => {
    const next = reduceSettings(SETTINGS, { kind: 'window', index: 1, time: '05:30' })
    expect(next?.window).toEqual(['01:00', '05:30'])
    expect(SETTINGS.window).toEqual(['01:00', '06:00'])
    expect(reduceSettings(SETTINGS, { kind: 'window', index: 0, time: '' })).toBeNull()
    expect(reduceSettings(SETTINGS, { kind: 'window', index: 0, time: '01:00' })).toBeNull()
  })

  test('models: one tier at a time, null = default', () => {
    const next = reduceSettings(SETTINGS, { kind: 'model', tier: 'tidy', model: 'anthropic/claude-haiku-4-5' })
    expect(next?.models).toEqual({ ...SETTINGS.models, tidy: 'anthropic/claude-haiku-4-5' })
    expect(reduceSettings(next!, { kind: 'model', tier: 'judge', model: null })?.models.judge).toBeNull()
    expect(reduceSettings(SETTINGS, { kind: 'model', tier: 'triage', model: null })).toBeNull()
  })

  test('digest day and time write the one string', () => {
    expect(reduceSettings(SETTINGS, { kind: 'digest_day', day: 'Sun' })?.digest).toBe('Sun 08:00')
    expect(reduceSettings(SETTINGS, { kind: 'digest_time', time: '19:30' })?.digest).toBe('Mon 19:30')
    expect(reduceSettings(SETTINGS, { kind: 'digest_time', time: '7' })).toBeNull()
    expect(reduceSettings(SETTINGS, { kind: 'digest_day', day: 'Mon' })).toBeNull()
  })

  test('pause is seven days from now; resume clears; enabled is never touched', () => {
    const now = new Date('2026-10-08T12:00:00Z')
    const paused = reduceSettings(SETTINGS, { kind: 'pause', now })!
    expect(paused.paused_until).toBe('2026-10-15T12:00:00.000Z')
    expect(paused.paused_until).toBe(pauseUntil(now))
    expect(isPaused(paused.paused_until, now)).toBe(true)
    expect(isPaused(paused.paused_until, new Date('2026-10-16T00:00:00Z'))).toBe(false)
    expect(isPaused(null, now)).toBe(false)
    expect(reduceSettings(paused, { kind: 'resume' })?.paused_until).toBeNull()
    expect(reduceSettings(SETTINGS, { kind: 'resume' })).toBeNull()
    expect(paused.enabled).toBe(SETTINGS.enabled)
    expect(isPaused(FIXTURES.paused.settings.paused_until, new Date())).toBe(true)
    expect(isPaused(FIXTURES.disabled.settings.paused_until, new Date())).toBe(false)
  })
})

describe('resolvedDefault (§9.2)', () => {
  const d = { title: 'anthropic/claude-haiku-4-5', subagent: 'anthropic/claude-fable-5-1', session: 'openai/gpt-5.6-sol' }
  test('tidy follows the title model, propose the subagent model, judge the other vendor', () => {
    expect(resolvedDefault('triage', d)).toBe('rules only')
    expect(resolvedDefault('tidy', d)).toBe('anthropic/claude-haiku-4-5')
    expect(resolvedDefault('propose', d)).toBe('anthropic/claude-fable-5-1')
    expect(resolvedDefault('judge', d)).toBe('openai/gpt-5.6-sol')
  })
  test('without helper models everything falls to the session default; same vendor → another seed', () => {
    const bare = { title: null, subagent: null, session: 'anthropic/claude-fable-5-1' }
    expect(resolvedDefault('tidy', bare)).toBe('anthropic/claude-fable-5-1')
    expect(resolvedDefault('judge', bare)).toBe('anthropic/claude-fable-5-1 (another seed)')
  })
})
