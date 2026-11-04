import { expect, test } from 'vitest'
import type { KernelState } from '@/rpc/bindings/KernelState'
import type { PassEnd } from '@/rpc/bindings/PassEnd'
import { digest } from './grammar'
import { followSuperseded, kernelLabel, runAllLabel, runAllPairs, type Step } from './RunbookBar'
import { RUNBOOK, SESSION, chatRef, run, spec, state } from './test-fixtures'

const steps = (states: Parameters<typeof state>[1][] = []): Step[] => [0, 1, 2].map((i) => ({ ref: chatRef(i + 1), spec: spec(i), state: state(chatRef(i + 1), states[i] ?? {}) }))

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
  const safe = [{ ...two[0]! }, { ...two[1]!, spec: { ...two[1]!.spec, options: { ...two[1]!.spec.options, danger: false } } }]
  expect(runAllLabel(safe).label).toBe('Run all')
  expect(runAllLabel(safe.slice(1)).label).toBe('Run all · stops before Apply to ir-prod1')
})

test('after a pass stops it says where from the engine’s pass (171F B11): at a failed step, or before a boundary', () => {
  const ok = { latest: run({ id: 'r_1', pass: 'p_1', cell: chatRef(1) }) }
  const bad = { latest: run({ id: 'r_2', pass: 'p_1', cell: chatRef(2), state: 'failed', exit: { type: 'code', value: 1 } }) }
  const at: PassEnd = { pass: 'p_1', total: 3, ran: 2, stopped: { cell: chatRef(2), why: 'exit 1 · expect “3 to update” missed', before: false } }
  const r = runAllLabel(steps([ok, bad]), at)
  expect(r.label).toBe('Run all · stopped at a failed step (2/3)')
  expect(r.stop?.reason).toBe('exit 1 · expect “3 to update” missed')
  expect(r.done).toBe(1)
  const before: PassEnd = { pass: 'p_2', total: 3, ran: 1, stopped: { cell: chatRef(2), why: 'a pipe into a shell', before: true } }
  expect(runAllLabel(steps([ok]), before).label).toBe('Run all · stopped before Preview (2/3)')
  // A clean end, or a pass that stopped in another reply, leaves the prediction.
  expect(runAllLabel(steps([ok]), { ...at, stopped: null }).label).toBe('Run all · stops before Apply to ir-prod1')
  expect(runAllLabel(steps([ok]), { ...at, stopped: { ...before.stopped!, cell: chatRef(1, '7') } }).label).toBe('Run all · stops before Apply to ir-prod1')
})

test('a replaced step follows its replacement (171F B8): counted when the replacement ran, and sent by its ref and digest', async () => {
  const replacement = chatRef(1, '99')
  const done = (ordinal: number) => ({ latest: run({ cell: chatRef(ordinal) }) })
  const old = steps([done(1), { superseded_by: replacement }, done(3)])
  // Replacement not read yet: skipped, as before.
  expect(runAllLabel(followSuperseded(old, [null, null, null], () => null))).toMatchObject({ done: 2, total: 2 })
  const next = state(replacement, { digest: 'd99', latest: run({ cell: replacement }) })
  const followed = followSuperseded(old, [null, next, null], () => null)
  expect(runAllLabel(followed)).toMatchObject({ done: 3, total: 3 })
  const pairs = await runAllPairs(followed, RUNBOOK)
  expect(pairs[1]).toEqual([replacement, 'd99'])
  expect(pairs[0]).toEqual([chatRef(1), await digest(RUNBOOK[0] as string)])
  // Every step replaced and none read: a disabled pointer to the newer version.
  const all = steps([{ superseded_by: replacement }, { superseded_by: replacement }, { superseded_by: replacement }])
  expect(runAllLabel(all)).toMatchObject({ label: 'Replaced by a newer version ↓', total: 0 })
})

test('the bar names the kernels its cells use, never another session kernel (171F B26)', () => {
  const k = (spec: KernelState['id']['spec'], phase: KernelState['phase']): KernelState => ({ id: { scope: { type: 'session', id: SESSION }, spec }, phase, label: '', cwd: null, env_warning: null })
  const sh = k({ type: 'shell', kind: 'sh', path: '/bin/sh' }, 'busy')
  const bash = k({ type: 'shell', kind: 'bash', path: '/opt/homebrew/bin/bash' }, 'ready')
  const python = k({ type: 'python', path: '/usr/bin/python3' }, 'busy')
  expect(kernelLabel(steps(), [sh, bash])).toBe('bash · idle')
  expect(kernelLabel(steps(), [sh])).toBe('bash')
  const py: Step = { ref: chatRef(4), spec: { ...spec(0), lang: 'python' }, state: null }
  expect(kernelLabel([...steps(), py], [sh, bash, python])).toBe('bash + python · busy')
  expect(kernelLabel([], [bash])).toBeNull()
})
