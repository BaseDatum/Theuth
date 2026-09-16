import { expect, test } from 'vitest'
import { diffLines } from './diff'

test('diffLines keeps context, marks the changed line and appended lines', () => {
  expect(diffLines('a\nb\nc', 'a\nB\nc\nd')).toEqual([
    { op: ' ', text: 'a' },
    { op: '-', text: 'b' },
    { op: '+', text: 'B' },
    { op: ' ', text: 'c' },
    { op: '+', text: 'd' },
  ])
})
