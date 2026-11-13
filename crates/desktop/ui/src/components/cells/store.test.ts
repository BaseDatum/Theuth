import { expect, test, vi } from 'vitest'
import type { PassEnd } from "@/rpc/bindings/method.cells.output"
import { RpcError } from '@/rpc/client'
import { cells, duration, exitText, lastPass, mintId, press, refKey, register, registeredCells, scopeKey } from './store'
import { dispatch } from '@/store/sessions'
import { SESSION, chatRef, fakeApi, run, spec, state } from './test-fixtures'

test('run ids are r_ + a 26-character Crockford ULID, time-ordered (§5.10)', () => {
  const a = mintId('r_', 1_762_180_000_000, new Uint8Array(10))
  const b = mintId('r_', 1_762_180_000_001, new Uint8Array(10).fill(255))
  expect(a).toMatch(/^r_[0-9A-HJKMNP-TV-Z]{26}$/)
  expect(a < b).toBe(true)
  expect(mintId('p_')).toMatch(/^p_[0-9A-HJKMNP-TV-Z]{26}$/)
  expect(mintId('r_')).not.toBe(mintId('r_'))
})

test('a transport failure retries the SAME run id; an engine answer is final', async () => {
  const ids: string[] = []
  let fails = 1
  const ok = await press(
    'r_A',
    async (id) => {
      ids.push(id)
      if (fails-- > 0) throw new Error('socket closed')
      return id
    },
    3,
    0,
  )
  expect(ok).toBe('r_A')
  expect(ids).toEqual(['r_A', 'r_A'])

  const refused: string[] = []
  await expect(
    press(
      'r_B',
      async (id) => {
        refused.push(id)
        throw new RpcError(-32020, 'this cell changed — look again')
      },
      3,
      0,
    ),
  ).rejects.toThrow('this cell changed — look again')
  expect(refused).toEqual(['r_B'])
})

test('durations read like the design: 2.1 s, 41 s, 2:14', () => {
  expect([duration(2.1), duration(41), duration(134)]).toEqual(['2.1 s', '41 s', '2:14'])
})

test('a watch-only register (171F B8) asks cell_state for that ref and never hides a mounted cell’s spec', async () => {
  const scope = { type: 'session', id: SESSION } as const
  const mine = chatRef(1, '7')
  const next = chatRef(1, '99')
  const pass: PassEnd = { pass: 'p_1', total: 2, ran: 1, stopped: { cell: next, why: 'exit 1', before: false } }
  const { api, calls } = fakeApi({ cell_state: () => ({ cells: [state(next, { latest: run({ cell: next, pass: 'p_1', state: 'failed' }) })], kernels: [], pass }) })
  const offMine = register(api, scope, { ref: mine, spec: spec(0), label: 'Export state' })
  const offNext = register(api, scope, { ref: next, spec: null, label: 'Preview' })
  const offAgain = register(api, scope, { ref: mine, spec: null, label: 'x' })
  await vi.waitFor(() => expect(cells.getState().scopes[scopeKey(scope)]?.pass).toEqual(pass))
  expect(calls.filter(([m]) => m === 'cell_state').at(-1)?.[1]).toEqual([scope, [mine, next]])
  expect(registeredCells(scope).find((c) => refKey(c.ref) === refKey(mine))).toMatchObject({ spec: spec(0), label: 'Export state', mounts: 2 })
  // The pass rides the record (171F B11) until a newer pass runs.
  const slice = cells.getState().scopes[scopeKey(scope)]!
  expect(lastPass(slice)).toEqual(pass)
  const running = { ...slice.states, x: state(mine, { latest: run({ cell: mine, pass: 'p_2', state: 'running' }) }) }
  expect(lastPass({ ...slice, states: running })).toBeNull()
  for (const off of [offMine, offNext, offAgain]) off()
  cells.setState({ scopes: {} })
})

test('171F B5: an ending state names itself before the exit code (timeout exits 130 too), like deliver::exit_word', () => {
  expect(exitText(run({ state: 'timeout', exit: { type: 'code', value: 130 } }))).toBe('timed out')
  expect(exitText(run({ state: 'killed', exit: { type: 'signal', value: 'SIGKILL' } }))).toBe('killed')
  expect(exitText(run({ state: 'lost', exit: null }))).toBe('lost')
  expect(exitText(run({ state: 'failed', exit: { type: 'code', value: 1 } }))).toBe('exit 1')
  expect(exitText(run({ state: 'stopped', exit: { type: 'signal', value: 'SIGINT' } }))).toBe('SIGINT')
  expect(exitText(run({ state: 'stopped', exit: null }))).toBe('stopped')
})

test('184 §6.5: CellLatestChanged for an open note re-reads cell_latest; other docs and sessions are left alone', async () => {
  const doc = 'dddddddddddddddddddddddddd'
  const scope = { type: 'doc', id: doc } as const
  const rows = [{ id: `${doc}:a`, engine: 'xeus1' }]
  const { api, calls } = fakeApi({ cell_state: () => ({ cells: [], kernels: [] }), cell_latest: () => rows })
  const off = register(api, scope, { ref: { type: 'note', doc, key: { kind: 'name', value: 'a' } }, spec: spec(0), label: 'a' })
  await vi.waitFor(() => expect(cells.getState().scopes[scopeKey(scope)]?.rows).toEqual(rows))
  const reads = () => calls.filter(([m]) => m === 'cell_latest').length
  const before = reads()
  dispatch([{ type: 'CellLatestChanged', doc: 'other' } as never])
  dispatch([{ type: 'CellLatestChanged', doc } as never])
  await vi.waitFor(() => expect(reads()).toBe(before + 1))
  off()
})
