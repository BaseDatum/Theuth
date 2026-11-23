import { expect, test } from 'vitest'
import { anchorLine, changedLines, COMPARE_LINES, HIGHLIGHT_CHARS, HIGHLIGHT_LINES, highlightedLines, langOf, linesOf } from './FileView'

test('the language comes from the event lang, else the file name or extension; unknown is plain', () => {
  expect(langOf('rs', '/a/main.rs')).toBe('rust')
  expect(langOf('tsx')).toBe('tsx')
  expect(langOf('Dockerfile', '/srv/Dockerfile')).toBe('docker')
  expect(langOf('makefile')).toBe('make')
  expect(langOf('justfile')).toBe('just')
  expect(langOf('', '/x/Cargo.TOML')).toBe('toml')
  expect(langOf(null, '/x/Makefile')).toBe('make')
  expect(langOf('weird', '/x/notes.xyz')).toBeNull()
  expect(langOf(undefined, '/x/LICENSE')).toBeNull()
})

test('a normal file is highlighted whole; a long or minified one only up to the line and character budgets', () => {
  expect(highlightedLines(linesOf('a\nb\nc\n'))).toBe(3)
  expect(highlightedLines(Array.from({ length: HIGHLIGHT_LINES + 10 }, () => 'x'))).toBe(HIGHLIGHT_LINES)
  const wide = 'y'.repeat(HIGHLIGHT_CHARS / 4)
  expect(highlightedLines([wide, wide, wide, wide, wide])).toBe(3)
  expect(highlightedLines(['z'.repeat(HIGHLIGHT_CHARS + 1)])).toBe(0)
})

test('a final newline closes the last line instead of opening an empty one', () => {
  expect(linesOf('')).toEqual([])
  expect(linesOf('a\r\nb\r\n')).toEqual(['a', 'b'])
  expect(linesOf('a\n\n')).toEqual(['a', ''])
})

test('changed lines are the new lines of the new text, numbered in it', () => {
  expect([...changedLines('a\nb\nc\n', 'a\nB\nc\n')!]).toEqual([2])
  expect([...changedLines('a\nb\n', 'x\na\nb\ny\n')!]).toEqual([1, 4])
  expect(changedLines('a\nb\n', 'a\n')!.size).toBe(0)
  expect(changedLines('same\n', 'same\n')!.size).toBe(0)
  const big = 'l\n'.repeat(COMPARE_LINES + 1)
  expect(changedLines(big, 'l\n')).toBeNull()
})

test('the scroll anchor follows the top line through insertions and deletions above it', () => {
  const before = 'a\nb\nc\nd\ne\n'
  expect(anchorLine(before, 'x\ny\na\nb\nc\nd\ne\n', 4)).toBe(6)
  expect(anchorLine(before, 'c\nd\ne\n', 4)).toBe(2)
  // The anchor line itself removed: where its run was.
  expect(anchorLine(before, 'a\nb\ne\n', 3)).toBe(3)
  expect(anchorLine(before, before, 5)).toBe(5)
  // Past the end of a shorter file it clamps to its last line.
  expect(anchorLine(before, 'a\n', 5)).toBe(1)
})
