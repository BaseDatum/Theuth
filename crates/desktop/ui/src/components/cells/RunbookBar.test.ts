import { expect, test } from 'vitest'
import { runAllLabel } from './RunbookBar'
import { chatRef, run, spec, state } from './test-fixtures'

const steps = (states: Parameters<typeof state>[1][] = []) => [0, 1, 2].map((i) => ({ spec: spec(i), state: state(chatRef(i + 1), states[i] ?? {}) }))

test('the §3.8 runbook: Run all stops before the danger apply, before any run (§7.2)', () => {
  expect(runAllLabel(steps()).label).toBe('Run all · stops before Apply to ir-prod1')
  expect(runAllLabel(steps()).done).toBe(0)
})

test('a blocking lint chip, a missing folder or an invalid cell is a boundary; superseded cells are skipped', () => {
  const lint = { chips: [{ kind: 'pipe_to_shell' as const, text: 'pipes a download into a shell', stops_run_all: true, count: null, name: null }] }
  expect(runAllLabel(steps([{}, lint])).label).toBe('Run all · stops before Preview')
  const cwd = { problems: [{ code: 'cwd_not_found', severity: 'warning' as const, message: 'not found', key: 'cwd' }] }
  expect(runAllLabel(steps([cwd])).label).toBe('Run all · stops before Export state')
  expect(runAllLabel(steps([{ superseded_by: chatRef(1, '9') }])).label).toBe('Run all · stops before Apply to ir-prod1')
})

test('an `after` gate on an earlier cell of the pass is not a boundary in advance; one on a cell elsewhere is', () => {
  const two = steps().slice(1)
  const safe = [{ ...two[0]! }, { spec: { ...two[1]!.spec, options: { ...two[1]!.spec.options, danger: false } }, state: two[1]!.state }]
  expect(runAllLabel(safe).label).toBe('Run all')
  expect(runAllLabel(safe.slice(1)).label).toBe('Run all · stops before Apply to ir-prod1')
})

test('after a pass stops at a failed step it says where, with the reason one click away', () => {
  const ok = { latest: run({ id: 'r_1', pass: 'p_1', cell: chatRef(1) }) }
  const bad = { latest: run({ id: 'r_2', pass: 'p_1', cell: chatRef(2), state: 'failed', exit: { type: 'code', value: 1 }, expect: [{ item: { kind: 'text', value: '3 to update' }, met: false }] }) }
  const r = runAllLabel(steps([ok, bad]))
  expect(r.label).toBe('Run all · stopped at a failed step (2/3)')
  expect(r.stop?.reason).toBe('exit 1 · expect “3 to update” missed')
  expect(r.done).toBe(1)
})
