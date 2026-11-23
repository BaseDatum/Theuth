import { expect, test } from 'vitest'
import { diffLayout } from './diff-layout'

test('a diff with no remembered choice is split only when wider than ~900px; a remembered choice wins at any width', () => {
  expect(diffLayout(null, 1200)).toBe('split')
  expect(diffLayout(null, 900)).toBe('unified')
  expect(diffLayout(null, 0)).toBe('unified')
  expect(diffLayout('unified', 1600)).toBe('unified')
  expect(diffLayout('split', 400)).toBe('split')
})
