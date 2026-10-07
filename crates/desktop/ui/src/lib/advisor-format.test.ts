import { expect, test } from 'vitest'
import { STATE_FG, TONE, cardTitle, contextLine, detailLine, pillLabel, tokensLine, worstSeverity } from './advisor-format'

const counts = (nit = 0, concern = 0, blocker = 0) => ({ nit, concern, blocker })

test('severity → tone: nit is info, concern warning, blocker danger; the worst note tints the card', () => {
  expect(TONE.nit.fg).toBe('text-info')
  expect(TONE.concern.fg).toBe('text-warning')
  expect(TONE.blocker.fg).toBe('text-danger')
  expect(worstSeverity([])).toBe('nit')
  expect(worstSeverity([{ severity: 'nit', note: 'a' }, { severity: 'blocker', note: 'b' }, { severity: 'concern', note: 'c' }])).toBe('blocker')
  expect(worstSeverity([{ severity: 'nit', note: 'a' }, { severity: 'concern', note: 'c' }])).toBe('concern')
})

test('pill label by state: idle, reviewing…, N notes, paused, halted; off has no pill', () => {
  expect(pillLabel({ state: 'idle', notes: counts() })).toBe('idle')
  expect(pillLabel({ state: 'idle', notes: counts(1) })).toBe('1 note')
  expect(pillLabel({ state: 'idle', notes: counts(1, 1) })).toBe('2 notes')
  expect(pillLabel({ state: 'reviewing', notes: counts(5) })).toBe('reviewing…')
  expect(pillLabel({ state: 'paused', notes: counts() })).toBe('paused')
  expect(pillLabel({ state: 'halted', notes: counts() })).toBe('halted')
  expect(pillLabel({ state: 'off', notes: counts() })).toBe('')
  expect(STATE_FG.halted).toBe('text-danger')
  expect(STATE_FG.paused).toBe('text-warning')
  expect(cardTitle(1)).toBe('Advisor · 1 note')
  expect(cardTitle(2)).toBe('Advisor · 2 notes')
})

test('context and token lines use k(); an unknown window shows no percentage', () => {
  expect(contextLine(38_200, 200_000)).toBe('38.2k / 200k · 19%')
  expect(contextLine(0, 0)).toBe('—')
  expect(contextLine(900, 0)).toBe('900')
  expect(tokensLine(1_234, 340)).toBe('1.2k in / 340 out')
})

test('detail line only when paused/halted, with a fallback reason', () => {
  expect(detailLine({ state: 'idle', detail: 'x' })).toBeUndefined()
  expect(detailLine({ state: 'paused', detail: null })).toBe('Advisor paused — usage limit')
  expect(detailLine({ state: 'halted', detail: 'invalid_request: tool advise' })).toBe('Advisor halted — invalid_request: tool advise')
})
