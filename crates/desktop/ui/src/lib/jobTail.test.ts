import { expect, test } from 'vitest'
import { JOB_LINES, pushChunk, rows } from './jobTail'

test('rewinds on \\r, strips escapes, caps the tail', () => {
  let lines = pushChunk([], 'a\nb\n\x1b[32mgreen\x1b[0m\x1b]0;title\x07\tx\n10%\r55%')
  expect(lines).toEqual(['a', 'b', 'green    x', '55%'])
  expect(rows(lines)).toEqual(['a', 'b', 'green    x', '55%'])
  lines = pushChunk(lines, '\n')
  expect(rows(lines)).toEqual(['a', 'b', 'green    x', '55%'])
  for (let i = 0; i < 20; i++) lines = pushChunk(lines, `${i}\n`)
  expect(rows(lines)).toHaveLength(JOB_LINES)
  expect(rows(lines)[0]).toBe('10')
  expect(rows(pushChunk(lines, 'tail'))).toHaveLength(JOB_LINES)
  expect(rows(pushChunk(lines, 'tail')).at(-1)).toBe('tail')
})
