import { expect, test } from 'vitest'
import { type DiffRow, marksOf, parseUnified, splitRows } from './editDiff'

const rowsOf = (patch: string[]): DiffRow[] => parseUnified(patch.join('\n')).files[0]!.rows
/** Each split row as `old | new` texts (`·` for filler), hunk headers and notes as themselves. */
const sides = (rows: DiffRow[]) =>
  splitRows(rows).map((s) => (s.kind !== 'line' ? `${s.kind}:${rows[s.row]!.text}` : `${s.left === null ? '·' : rows[s.left]!.text} | ${s.right === null ? '·' : rows[s.right]!.text}`))

const patch = [
  '--- a/f.txt',
  '+++ b/f.txt',
  '@@ -1,7 +1,6 @@',
  ' keep',
  '-a1',
  '-a2',
  '-a3',
  '+b1',
  ' mid',
  '-c1',
  '+d1',
  '+d2',
  ' end',
  '@@ -20,1 +20,1 @@',
  '-x',
  '+y',
]

test('split: removals pair in order with the additions right after them; the longer run ends against filler', () => {
  expect(sides(rowsOf(patch))).toEqual([
    'hunk:@@ -1,7 +1,6 @@',
    'keep | keep',
    'a1 | b1',
    'a2 | ·',
    'a3 | ·',
    'mid | mid',
    'c1 | d1',
    '· | d2',
    'end | end',
    'hunk:@@ -20,1 +20,1 @@',
    'x | y',
  ])
})

test('split: a context row is one row index on both sides, so each side reads its own line number', () => {
  const rows = rowsOf(patch)
  const keep = splitRows(rows)[1]!
  expect(keep).toEqual({ kind: 'line', left: 1, right: 1 })
  const pair = splitRows(rows)[2]
  expect(pair).toMatchObject({ kind: 'line' })
  if (pair?.kind !== 'line') throw new Error('not a line')
  expect(rows[pair.left!]).toMatchObject({ op: '-', old: 2 })
  expect(rows[pair.right!]).toMatchObject({ op: '+', next: 2 })
})

test('split: pure additions or removals stand alone; a run never pairs across a hunk header or context', () => {
  const rows = rowsOf(['--- a/f', '+++ b/f', '@@ -1,2 +1,2 @@', '-gone', ' ctx', '+new', '@@ -9,1 +9,0 @@', '-tail'])
  expect(sides(rows)).toEqual(['hunk:@@ -1,2 +1,2 @@', 'gone | ·', 'ctx | ctx', '· | new', 'hunk:@@ -9,1 +9,0 @@', 'tail | ·'])
})

test('split: a note inside a run follows the run, and the run still pairs across it', () => {
  const rows: DiffRow[] = [
    { kind: 'line', op: '-', text: 'old', old: 1 },
    { kind: 'note', text: '\\ No newline at end of file' },
    { kind: 'line', op: '+', text: 'new', next: 1 },
    { kind: 'note', text: '\\ No newline at end of file' },
  ]
  expect(sides(rows)).toEqual(['old | new', 'note:\\ No newline at end of file', 'note:\\ No newline at end of file'])
})

test('word marks go to exactly the split pairs: both sides of a pair, never an unpaired or context row', () => {
  const rows = rowsOf(patch)
  const marks = marksOf(rows)
  const marked = [...marks.keys()].map((i) => rows[i]!.text).sort()
  expect(marked).toEqual(['a1', 'b1', 'c1', 'd1', 'x', 'y'])
  expect(marks.get(rows.findIndex((r) => r.text === 'c1'))?.some((p) => p.changed)).toBe(true)
})
