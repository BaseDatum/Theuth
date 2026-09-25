import { expect, test } from 'vitest'
import { movedTab } from './moved'

test('the three moved settings tabs redirect to their Library routes; other tabs stay', () => {
  expect(movedTab('skills')).toBe('/skills')
  expect(movedTab('tools')).toBe('/tools')
  expect(movedTab('secrets')).toBe('/secrets')
  expect(movedTab('general')).toBe(null)
  expect(movedTab('constructor')).toBe(null)
})
