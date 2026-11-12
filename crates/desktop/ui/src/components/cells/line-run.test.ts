import { expect, test, vi } from 'vitest'
import type { Cell as CellSpec } from '@/rpc/bindings/auxiliary.surface.output'
import { cellOf } from './grammar'
import { askSkipped, busy, cellIdentity, hoverAfter, lineGuard, noteHover, pastGate, pressed, refused, settled, skipAsking, started, WARN_MS } from './line-run'
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

test('CELLAFTER: a ► on a disarmed cell warns first; the same line again within the window runs, later or elsewhere warns again', () => {
  const first = pastGate(null, 2, T)
  expect(first).toEqual({ run: false, warned: { line: 2, at: T } })
  expect(pastGate(first.warned, 2, T + WARN_MS)).toEqual({ run: true, warned: null })
  expect(pastGate(first.warned, 2, T + WARN_MS + 1)).toEqual({ run: false, warned: { line: 2, at: T + WARN_MS + 1 } })
  // Another line resets the warning to that line; its own second press runs.
  const other = pastGate(first.warned, 3, T + 100)
  expect(other).toEqual({ run: false, warned: { line: 3, at: T + 100 } })
  expect(pastGate(other.warned, 2, T + 200).run).toBe(false)
  expect(pastGate(other.warned, 3, T + 200).run).toBe(true)
})

test('CELLAFTER: a danger ► asks (naming the gate when also disarmed) until "Don’t ask again"; a disarmed one warns; else it runs', () => {
  const why = 'after: preview — run it first'
  expect(lineGuard(true, false, null)).toEqual({ kind: 'confirm', reason: null })
  expect(lineGuard(true, false, why)).toEqual({ kind: 'confirm', reason: why })
  expect(lineGuard(true, true, null)).toEqual({ kind: 'run' })
  // Not asking any more never waives the gate's own warning.
  expect(lineGuard(true, true, why)).toEqual({ kind: 'warn', text: `${why} — press ► again to run anyway` })
  expect(lineGuard(false, false, why)).toEqual({ kind: 'warn', text: `${why} — press ► again to run anyway` })
  expect(lineGuard(false, false, null)).toEqual({ kind: 'run' })
  // A reason's own "— run it again" gives way to the offer; the confirm keeps the reason whole.
  const old = 'the prepare run is more than 1 h old — run it again'
  expect(lineGuard(false, false, old)).toEqual({ kind: 'warn', text: 'the prepare run is more than 1 h old — press ► again to run anyway' })
  expect(lineGuard(true, false, old)).toEqual({ kind: 'confirm', reason: old })
})

test('CELLAFTER: "Don’t ask again" is per cell identity — scope + name across edits and refs, else the ref', () => {
  const store = new Map<string, string>()
  vi.stubGlobal('localStorage', { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => void store.set(k, v) })
  try {
    const named = cellOf('bash', '#| name: wipe\n#| danger\necho a') as CellSpec
    const edited = cellOf('bash', '#| name: wipe\n#| danger\necho b') as CellSpec
    const unnamed = cellOf('bash', '#| danger\necho a') as CellSpec
    const byName = cellIdentity({ type: 'note', doc: 'd1', key: { kind: 'name', value: 'wipe' } }, named)
    expect(cellIdentity({ type: 'note', doc: 'd1', key: { kind: 'block', value: 'b7' } }, edited)).toBe(byName)
    expect(cellIdentity({ type: 'note', doc: 'd2', key: { kind: 'name', value: 'wipe' } }, named)).not.toBe(byName)
    const block = cellIdentity({ type: 'note', doc: 'd1', key: { kind: 'block', value: 'b7' } }, unnamed)
    expect(askSkipped(byName)).toBe(false)
    skipAsking(byName)
    expect([askSkipped(byName), askSkipped(block)]).toEqual([true, false])
  } finally {
    vi.unstubAllGlobals()
  }
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
