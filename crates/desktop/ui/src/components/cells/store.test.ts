import { expect, test } from 'vitest'
import { RpcError } from '@/rpc/client'
import { duration, exitText, mintId, press } from './store'
import { run } from './test-fixtures'

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

test('171F B5: an ending state names itself before the exit code (timeout exits 130 too), like deliver::exit_word', () => {
  expect(exitText(run({ state: 'timeout', exit: { type: 'code', value: 130 } }))).toBe('timed out')
  expect(exitText(run({ state: 'killed', exit: { type: 'signal', value: 'SIGKILL' } }))).toBe('killed')
  expect(exitText(run({ state: 'lost', exit: null }))).toBe('lost')
  expect(exitText(run({ state: 'failed', exit: { type: 'code', value: 1 } }))).toBe('exit 1')
  expect(exitText(run({ state: 'stopped', exit: { type: 'signal', value: 'SIGINT' } }))).toBe('SIGINT')
  expect(exitText(run({ state: 'stopped', exit: null }))).toBe('stopped')
})
