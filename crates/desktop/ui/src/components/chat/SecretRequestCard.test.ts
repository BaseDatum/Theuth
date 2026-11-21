import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { beforeAll, expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import { SecretRequestCard, answerPayload, eventFields } from './SecretRequestCard'
import { choiceFromSuggestion, describeExpiry, formatExpiry, localInputToMs, msToLocalInput, resolveExpiry } from './secret-expiry'

vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => ({}) }))

const NOW = new Date(2026, 10, 21, 13, 0).getTime()
// The card reads the clock for "In 2 hours" and the past check.
beforeAll(() => {
  vi.useFakeTimers()
  vi.setSystemTime(NOW)
})
const H = 3_600_000
const FORM = [
  { name: 'gh.user', label: 'Username', masked: false, current: null },
  { name: 'gh.pass', label: 'Password', masked: true, current: 2, note: "alice's GitHub password" },
]

const card = (props: Record<string, unknown>) => renderToStaticMarkup(createElement(SecretRequestCard, { id: 'r1', fields: FORM, ...props } as Parameters<typeof SecretRequestCard>[0]))

test('several fields render on one card, each with its own input and mask', () => {
  const html = card({ title: 'Log in to GitHub' })
  expect(html).toContain('Log in to GitHub')
  expect(html).toContain('aria-label="value of gh.user"')
  expect(html).toContain('aria-label="value of gh.pass"')
  expect(html).toMatch(/type="text"[^>]*aria-label="value of gh.user"/)
  expect(html).toMatch(/type="password"[^>]*aria-label="value of gh.pass"/)
  expect(html).toContain('adds v3')
  expect(html).toContain('alice&#x27;s GitHub password')
  // No suggestion: the control starts at Never.
  expect(html).toContain('Kept until you delete it')
})

test('they are answered together, with one expiry for every value; decline sends nothing', () => {
  const at = NOW + 2 * H
  expect(answerPayload(FORM, { 'gh.user': 'alice', 'gh.pass': 'pw' }, false, at)).toEqual([null, { 'gh.user': 'alice', 'gh.pass': 'pw' }, at])
  expect(answerPayload(FORM, { 'gh.user': 'alice' }, false)).toEqual([null, { 'gh.user': 'alice', 'gh.pass': '' }, null])
  expect(answerPayload(FORM, { 'gh.user': 'alice' }, true, at)).toEqual([null, null, null])
  const token = { name: 'GITHUB_TOKEN', label: 'token', masked: true, current: null }
  expect(answerPayload([token], { GITHUB_TOKEN: 'ghp_x' }, false, at)).toEqual(['ghp_x', null, at])
})

test('the three modes resolve to the right absolute time', () => {
  expect(resolveExpiry({ mode: 'never' }, NOW)).toEqual({ ok: true, at: null })
  expect(resolveExpiry({ mode: 'in', amount: '30', unit: 'minutes' }, NOW)).toEqual({ ok: true, at: NOW + 30 * 60_000 })
  expect(resolveExpiry({ mode: 'in', amount: '2', unit: 'hours' }, NOW)).toEqual({ ok: true, at: NOW + 2 * H })
  expect(resolveExpiry({ mode: 'in', amount: '3', unit: 'days' }, NOW)).toEqual({ ok: true, at: NOW + 72 * H })
  expect(resolveExpiry({ mode: 'on', local: '2026-11-22T14:00' }, NOW)).toEqual({ ok: true, at: new Date(2026, 10, 22, 14, 0).getTime() })
  expect(formatExpiry(new Date(2026, 10, 22, 14, 0).getTime(), NOW, 'en-GB')).toBe('Sun 22 Nov, 14:00')
  expect(formatExpiry(new Date(2027, 0, 3, 9, 5).getTime(), NOW, 'en-GB')).toBe('Sun 3 Jan 2027, 09:05')
})

test('a past time, zero, a negative or fractional number and an empty date are rejected inline', () => {
  for (const amount of ['0', '-1', '1.5', '', 'x']) expect(resolveExpiry({ mode: 'in', amount, unit: 'hours' }, NOW).ok, amount).toBe(false)
  expect(resolveExpiry({ mode: 'on', local: '2026-11-21T12:59' }, NOW)).toEqual({ ok: false, error: 'That time is in the past' })
  expect(resolveExpiry({ mode: 'on', local: msToLocalInput(NOW) }, NOW).ok).toBe(false)
  expect(resolveExpiry({ mode: 'on', local: '' }, NOW).ok).toBe(false)
  expect(localInputToMs('2026-02-30T10:00')).toBeNull()
  const html = card({ expires: '2020-01-01T00:00:00Z' })
  expect(html).toContain('That time is in the past')
})

test("the agent's suggestion pre-fills the control; the card shows where it lands", () => {
  expect(choiceFromSuggestion('30m')).toEqual({ mode: 'in', amount: '30', unit: 'minutes' })
  expect(choiceFromSuggestion('2h')).toEqual({ mode: 'in', amount: '2', unit: 'hours' })
  expect(choiceFromSuggestion('3d')).toEqual({ mode: 'in', amount: '3', unit: 'days' })
  const at = new Date(2026, 11, 1, 18, 30).getTime()
  expect(choiceFromSuggestion(new Date(at).toISOString())).toEqual({ mode: 'on', local: '2026-12-01T18:30' })
  expect(choiceFromSuggestion(null)).toEqual({ mode: 'never' })
  expect(choiceFromSuggestion('nonsense')).toEqual({ mode: 'never' })
  const html = card({ expires: '2h' })
  expect(html).toMatch(/aria-label="expires in"[^>]*value="2"|value="2"[^>]*aria-label="expires in"/)
  expect(html).toMatch(/<option value="hours" selected/)
  expect(html).toContain('Expires ')
  expect(html).toContain('then deleted')
  const on = card({ expires: new Date(at).toISOString() })
  expect(on).toContain('value="2026-12-01T18:30"')
})

test('a local event from an engine older than forms becomes one field; a form keeps its fields', () => {
  expect(eventFields({ name: 'TOKEN', current: 1, fields: [] })).toEqual([{ name: 'TOKEN', label: 'TOKEN', masked: true, current: 1 }])
  expect(eventFields({ name: 'gh.user', current: null, fields: FORM })).toBe(FORM)
})

test('lists say how long is left', () => {
  expect(describeExpiry(new Date(NOW + 40 * 60_000).toISOString(), NOW)).toBe('expires in 40 min')
  expect(describeExpiry(new Date(NOW + 3 * H).toISOString(), NOW)).toBe('expires in 3 h')
  expect(describeExpiry(new Date(NOW + 72 * H).toISOString(), NOW)).toBe(`expires ${formatExpiry(NOW + 72 * H, NOW)}`)
  expect(describeExpiry(new Date(NOW - 1).toISOString(), NOW)).toBe('expired')
})
