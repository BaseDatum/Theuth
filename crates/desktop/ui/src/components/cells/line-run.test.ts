import { expect, test } from 'vitest'
import { busy, hoverAfter, noteHover, pressed, refused, settled, started } from './line-run'
import { run } from './test-fixtures'

const ID = 'r_01JC7Q2M8V0000000000000001'
const T = 1_762_180_100_000
const mine = (over: Parameters<typeof run>[0] = {}) => run({ id: ID, mode: { type: 'line', line: 3 }, queued_ms: T, output_bytes: 0, ...over })

test('171F4 T2-6: a quiet line run shows running, then ✓ for exit 0 or ✗ otherwise', () => {
  const older = run({ queued_ms: T - 60_000 })
  let m = pressed(null, 3)
  expect(m).toEqual({ line: 3, phase: 'running', run: null, queued_ms: null })
  // Until `cell_run` answers, the cell's latest is the previous run: still running.
  expect(settled(m, older)).toBe(m)
  m = started(m, 3, mine({ state: 'queued', exit: null, ended_ms: null }))
  expect(m).toMatchObject({ phase: 'running', run: ID, queued_ms: T })
  expect(settled(m, older)).toBe(m)
  expect(settled(m, mine({ state: 'running', exit: null, ended_ms: null }))).toBe(m)
  expect(settled(m, mine())).toMatchObject({ line: 3, phase: 'ok' })
  expect(settled(m, mine({ output_bytes: null }))).toMatchObject({ phase: 'ok' })
  expect(settled(m, mine({ state: 'failed', exit: { type: 'code', value: 1 } }))).toMatchObject({ phase: 'fail' })
  expect(settled(m, mine({ state: 'killed', exit: { type: 'signal', value: 'SIGKILL' } }))).toMatchObject({ phase: 'fail' })
})

test('171F4 T2-6: a press while that line runs is ignored; another line, or a press after the ✓, presses', () => {
  const m = started(pressed(null, 3), 3, mine({ state: 'running', exit: null, ended_ms: null }))
  expect(busy(m, 3)).toBe(true)
  expect(pressed(m, 3)).toBe(m)
  expect(pressed(m, 4)).toEqual({ line: 4, phase: 'running', run: null, queued_ms: null })
  const done = settled(m, mine())
  expect(busy(done, 3)).toBe(false)
  expect(pressed(done, 3)).toMatchObject({ line: 3, phase: 'running', run: null })
})

test('171F4 T2-6: output, a newer run or a refused press clears the mark', () => {
  const m = started(pressed(null, 3), 3, mine({ state: 'running', exit: null, ended_ms: null }))
  // Printed: the output pane answers, the gutter goes back to ►.
  expect(settled(m, mine({ output_bytes: 12 }))).toBeNull()
  // The Run button (or Run all) queued a newer run of the cell.
  expect(settled(m, run({ id: 'r_newer', queued_ms: T + 1 }))).toBeNull()
  // The engine refused the press: the notice explains, no mark stays behind.
  expect(refused(pressed(null, 3), 3)).toBeNull()
  // A late answer for a mark already replaced changes nothing.
  const other = pressed(null, 5)
  expect(started(other, 3, mine())).toBe(other)
  expect(refused(other, 3)).toBe(other)
})

test('171F4 N2: in a note the pointer may travel left to the ► — the band before the text keeps the hovered line', () => {
  const TEXT_LEFT = 342
  let line = noteHover(null, 450, TEXT_LEFT, 2)
  expect(line).toBe(2)
  // In the band WebKit's caretRangeFromPoint answers a header offset (no code line): the ► stays on line 2.
  line = noteHover(line, 330, TEXT_LEFT, null)
  expect(line).toBe(2)
  line = noteHover(line, 316, TEXT_LEFT, 1)
  expect(line).toBe(2)
  // Back over the text: the line under the pointer, none on a header line.
  expect(noteHover(line, 400, TEXT_LEFT, 3)).toBe(3)
  expect(noteHover(line, 400, TEXT_LEFT, null)).toBeNull()
  // The band never invents a hover.
  expect(noteHover(null, 330, TEXT_LEFT, 1)).toBeNull()
  // Leaving the slot clears it (the same rule as the chat gutter).
  expect(hoverAfter(2, { kind: 'leave', inEditor: false })).toBeNull()
})
