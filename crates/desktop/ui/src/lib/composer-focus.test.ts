import { expect, test } from 'vitest'
import { shouldRefocus } from './composer-focus'

/** A stand-in element: `contains` is the only DOM the rule reads. */
const el = (...inside: object[]): Element => ({ contains: (n: Node | null) => n !== null && inside.includes(n) }) as unknown as Element

test('shouldRefocus: nothing focused (body/null) or the composer’s own box → yes; another input or dialog → no; touch → never', () => {
  const body = el()
  const button = el()
  const box = el(button)
  const other = el()
  expect(shouldRefocus(null, box, false, body)).toBe(true)
  expect(shouldRefocus(body, box, false, body)).toBe(true)
  expect(shouldRefocus(button, box, false, body)).toBe(true)
  expect(shouldRefocus(other, box, false, body)).toBe(false)
  expect(shouldRefocus(other, null, false, body)).toBe(false)
  // A phone keeps its keyboard down whatever has focus.
  expect(shouldRefocus(null, box, true, body)).toBe(false)
  expect(shouldRefocus(button, box, true, body)).toBe(false)
})
