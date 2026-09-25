import { expect, test } from 'vitest'
import { type Draft, emptyDraft, firstInvalid, starterCharter, suggestName, toFile, validate } from './wizard'

const draft = (x: Partial<Draft> = {}): Draft => ({ ...emptyDraft(), name: 'ops', title: 'Ops', body: 'You run prod.', ...x })
const roster = ['theuth', 'cto', 'sre']

test('identity: name is [a-z0-9-]{1,32}, not theuth, not taken; title required', () => {
  expect(validate('identity', draft(), roster)).toEqual({})
  expect(validate('identity', draft({ name: 'Ops' }), roster).name).toMatch(/lowercase/)
  expect(validate('identity', draft({ name: 'a'.repeat(33) }), roster).name).toBeDefined()
  expect(validate('identity', draft({ name: '' }), roster).name).toBeDefined()
  expect(validate('identity', draft({ name: 'theuth' }), roster).name).toMatch(/built-in/)
  expect(validate('identity', draft({ name: 'sre' }), roster).name).toMatch(/already/)
  expect(validate('identity', draft({ title: '  ' }), roster).title).toBeDefined()
})

test('role: manager must be theuth or on the roster, never itself', () => {
  expect(validate('role', draft({ reports_to: 'cto' }), roster)).toEqual({})
  expect(validate('role', draft({ reports_to: 'theuth' }), [])).toEqual({})
  expect(validate('role', draft({ reports_to: 'ghost' }), roster).reports_to).toBeDefined()
  expect(validate('role', draft({ name: 'cto', reports_to: 'cto' }), roster).reports_to).toMatch(/itself/)
})

test('budget: monthly needs an amount above zero and a 3-letter currency; none is always fine', () => {
  expect(validate('budget', draft(), roster)).toEqual({})
  expect(validate('budget', draft({ budget: { type: 'monthly', amount: 0, currency: 'USD' } }), roster).budget).toMatch(/above zero/)
  expect(validate('budget', draft({ budget: { type: 'monthly', amount: 5, currency: 'us' } }), roster).budget).toMatch(/currency/)
  expect(validate('budget', draft({ budget: { type: 'monthly', amount: 5, currency: 'EUR' } }), roster)).toEqual({})
})

test('charter cannot be empty; review aggregates every step; firstInvalid points at the earliest', () => {
  expect(validate('charter', draft({ body: ' \n' }), roster).body).toBeDefined()
  const bad = draft({ name: 'Bad Name', reports_to: 'ghost', body: '' })
  expect(Object.keys(validate('review', bad, roster)).sort()).toEqual(['body', 'name', 'reports_to'])
  expect(firstInvalid(bad, roster)).toBe('identity')
  expect(firstInvalid(draft({ body: '' }), roster)).toBe('charter')
  expect(firstInvalid(draft(), roster)).toBeNull()
})

test('toFile trims, nulls empty optionals, and sets paused per the Hire button', () => {
  const f = toFile(draft({ title: ' Ops ', capabilities: '  ', model: '', effort: '' }), true)
  expect(f).toMatchObject({ name: 'ops', title: 'Ops', capabilities: null, model: null, effort: null, tools: null, paused: true, proposed_by: null })
  expect(toFile(draft(), false).paused).toBe(false)
})

test('suggestName slugs a title; starter charter names the role and the manager', () => {
  expect(suggestName('Chief of Staff!')).toBe('chief-of-staff')
  expect(suggestName('  ')).toBe('')
  expect(suggestName('x'.repeat(40))).toHaveLength(32)
  const c = starterCharter(draft({ title: 'SRE', reports_to: 'cto', capabilities: 'incidents' }))
  expect(c).toContain('# SRE')
  expect(c).toContain('reporting to cto')
  expect(c).toContain('## When to hand off to you\n\nincidents')
})
