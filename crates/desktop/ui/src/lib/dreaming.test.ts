import { describe, expect, test } from 'vitest'
import { FIXTURES, NIGHT, QUIET_NIGHT, RESOLVED, RUNNING_NIGHT, SETTINGS, UNDONE_NIGHT } from '@/dev/dreaming-fixture'
import { BUDGET_PROBLEM, MAX_BUDGET_USD, budgetFromInput, countsLine, dreamNowBlocker, formatDigest, isPaused, nightLine, nightText, parseDigest, pauseUntil, reduceSettings, resolvedDefault, undoLine, usd } from './dreaming'

describe('formatting', () => {
  test('counts line omits zeros, pluralises, and says so when nothing happened', () => {
    expect(countsLine(NIGHT.counts)).toBe('212 retitled, 14 merged, 3 superseded, 31 archived, 15 verified, 9 cases, 1 skill edit kept, 2 rejected, 1 proposal')
    expect(countsLine({ ...NIGHT.counts, edits_accepted: 2n, cases: 1n })).toContain('1 case, 2 skill edits kept')
    expect(countsLine(QUIET_NIGHT.counts)).toBe('nothing to tidy')
  })

  test('night line per fixture state: none, a night, an undone night, a running one', () => {
    expect(nightLine(FIXTURES.empty.status.last_night)).toBe('no nights yet')
    expect(nightLine(FIXTURES.night.status.last_night)).toBe(`last night: ${countsLine(NIGHT.counts)}, $4.10`)
    expect(nightLine(FIXTURES.paused.status.last_night)).toBe(`last night: ${countsLine(UNDONE_NIGHT.counts)}, $0.93 (undone)`)
    expect(nightText(FIXTURES.disabled.status.last_night)).toBe('nothing to tidy, $0.00')
    expect(nightText(RUNNING_NIGHT)).toBe('40 retitled, $0.27')
    expect(nightText(null)).toBe('no nights yet')
  })

  test('usd renders two decimals everywhere (R-48): status card, undo list, Routines subtitle', () => {
    expect(usd(1)).toBe('$1.00')
    expect(usd(4.1)).toBe('$4.10')
    expect(usd(0.005)).toBe('$0.01')
    expect(usd(0.004)).toBe('$0.00')
    expect(usd(MAX_BUDGET_USD)).toBe('$1,000.00')
    expect(BUDGET_PROBLEM).toBe('Enter an amount above $0 and at most $1,000.00.')
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
  test('budget: within (0, 1000]; empty, zero, over the cap, negatives and no-ops return null (R-34)', () => {
    expect(reduceSettings(SETTINGS, { kind: 'budget', usd: 2.5 })?.budget_usd).toBe(2.5)
    expect(reduceSettings(SETTINGS, { kind: 'budget', usd: 0.01 })?.budget_usd).toBe(0.01)
    expect(reduceSettings(SETTINGS, { kind: 'budget', usd: 1000 })?.budget_usd).toBe(1000)
    expect(reduceSettings(SETTINGS, { kind: 'budget', usd: 0 })).toBeNull()
    expect(reduceSettings(SETTINGS, { kind: 'budget', usd: 1000.01 })).toBeNull()
    expect(reduceSettings(SETTINGS, { kind: 'budget', usd: 1 })).toBeNull()
    expect(reduceSettings(SETTINGS, { kind: 'budget', usd: -1 })).toBeNull()
    expect(reduceSettings(SETTINGS, { kind: 'budget', usd: Number.NaN })).toBeNull()
  })

  test('budgetFromInput: a cleared field is null, never $0; the Dream now dialog shares the rule', () => {
    expect(budgetFromInput('')).toBeNull()
    expect(budgetFromInput('   ')).toBeNull()
    expect(budgetFromInput('0')).toBeNull()
    expect(budgetFromInput('-2')).toBeNull()
    expect(budgetFromInput('abc')).toBeNull()
    expect(budgetFromInput('1e9')).toBeNull()
    expect(budgetFromInput('0.5')).toBe(0.5)
    expect(budgetFromInput('1000')).toBe(1000)
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

describe('dreamNowBlocker (R-46)', () => {
  const now = new Date('2026-10-08T12:00:00Z')
  const live = { enabled: true, routineEnabled: true, pausedUntil: null, running: false }
  test('runs only when enabled, switched on, not paused and no night in flight; the first reason wins', () => {
    expect(dreamNowBlocker(live, now)).toBeNull()
    expect(dreamNowBlocker({ ...live, routineEnabled: undefined }, now)).toBeNull()
    expect(dreamNowBlocker({ ...live, enabled: false }, now)).toMatch(/off in config/)
    expect(dreamNowBlocker({ ...live, routineEnabled: false }, now)).toMatch(/routine is off/)
    expect(dreamNowBlocker({ ...live, pausedUntil: '2026-10-13T12:00:00Z' }, now)).toMatch(/^Dreaming is paused until /)
    expect(dreamNowBlocker({ ...live, pausedUntil: '2026-10-01T12:00:00Z' }, now)).toBeNull()
    expect(dreamNowBlocker({ ...live, running: true }, now)).toBe('A night is already running.')
    expect(dreamNowBlocker({ ...live, enabled: false, running: true }, now)).toMatch(/off in config/)
  })
})

describe('resolvedDefault (§9.2, R-14)', () => {
  const d = { title: 'anthropic/claude-haiku-4-5', subagent: 'anthropic/claude-fable-5-1', session: 'openai/gpt-5.6-sol' }
  test('the server’s resolved tiers win when present; judge = propose is marked', () => {
    expect(resolvedDefault('triage', { ...d, resolved: RESOLVED })).toEqual({ model: null, same: false })
    expect(resolvedDefault('tidy', { ...d, resolved: RESOLVED })).toEqual({ model: 'anthropic/claude-haiku-4-5', same: false })
    expect(resolvedDefault('judge', { ...d, resolved: RESOLVED })).toEqual({ model: 'openai/gpt-5.6-sol', same: false })
    expect(resolvedDefault('judge', { ...d, resolved: { ...RESOLVED, judge: RESOLVED.propose } })).toEqual({ model: RESOLVED.propose, same: true })
    expect(resolvedDefault('tidy', { ...d, resolved: RESOLVED, routine: 'x/y' })).toEqual({ model: 'anthropic/claude-haiku-4-5', same: false })
  })
  test('without it the routine file’s model: is Tidy’s truth, then the title model; propose the subagent model; judge the other vendor', () => {
    expect(resolvedDefault('triage', d)).toEqual({ model: null, same: false })
    expect(resolvedDefault('tidy', { ...d, routine: 'openai/gpt-5.6-sol' })).toEqual({ model: 'openai/gpt-5.6-sol', same: false })
    expect(resolvedDefault('tidy', d)).toEqual({ model: 'anthropic/claude-haiku-4-5', same: false })
    expect(resolvedDefault('propose', d)).toEqual({ model: 'anthropic/claude-fable-5-1', same: false })
    expect(resolvedDefault('judge', d)).toEqual({ model: 'openai/gpt-5.6-sol', same: false })
  })
  test('without helper models everything falls to the session default; same vendor → the same model judges', () => {
    const bare = { title: null, subagent: null, session: 'anthropic/claude-fable-5-1' }
    expect(resolvedDefault('tidy', bare)).toEqual({ model: 'anthropic/claude-fable-5-1', same: false })
    expect(resolvedDefault('judge', bare)).toEqual({ model: 'anthropic/claude-fable-5-1', same: true })
  })
})
