import { expect, test } from 'vitest'
import { movedTab } from './moved'

test('the moved settings tabs redirect to their Library routes; other tabs stay', () => {
  expect(movedTab('skills')).toBe('/skills')
  expect(movedTab('secrets')).toBe('/secrets')
  expect(movedTab('tools')).toBe(null)
  expect(movedTab('general')).toBe(null)
  expect(movedTab('constructor')).toBe(null)
})
