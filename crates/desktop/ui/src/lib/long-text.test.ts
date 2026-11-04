import { expect, test } from 'vitest'
import { longText, showAll } from './long-text'

const lines = (n: number, sep = '\n') => Array.from({ length: n }, (_, i) => `line ${i}`).join(sep)

test('171F B6: long is more than 12 lines or more than 1200 characters', () => {
  expect(longText(lines(12))).toBeNull()
  expect(longText(lines(13))).toEqual({ lines: 13, chars: lines(13).length })
  expect(longText('x'.repeat(1200))).toBeNull()
  expect(longText('x'.repeat(1201))).toEqual({ lines: 1, chars: 1201 })
  // A CRLF is one break, not two lines.
  expect(longText(lines(12, '\r\n'))).toBeNull()
  expect(longText(lines(13, '\r\n'))?.lines).toBe(13)
  expect(longText('')).toBeNull()
})

test('171F B6: the toggle names lines, or characters for one long line', () => {
  const one = longText('y'.repeat(5000))
  expect(one).toEqual({ lines: 1, chars: 5000 })
  expect(one && showAll(one)).toBe('Show all · 5000 characters')
  expect(showAll({ lines: 40, chars: 900 })).toBe('Show all 40 lines')
})
