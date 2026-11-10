import { expect, test } from 'vitest'
import { RpcError } from '@/rpc/client'
import { awaitsRestart } from './Autoresearch'

const running = { isSuccess: true, error: null }
const off = { isSuccess: false, error: new RpcError(-32001, 'autoresearch is not running') }

test('188 F3: a saved value the engine does not run waits for a restart; one it runs does not', () => {
  expect(awaitsRestart(true, off)).toBe(true)
  expect(awaitsRestart(false, running)).toBe(true)
  expect(awaitsRestart(true, running)).toBe(false)
  expect(awaitsRestart(false, off)).toBe(false)
})

test('188 F3: unknown until both sides answered — settings loading, the probe loading or failing for another reason', () => {
  expect(awaitsRestart(undefined, off)).toBeNull()
  expect(awaitsRestart(true, { isSuccess: false, error: null })).toBeNull()
  expect(awaitsRestart(true, { isSuccess: false, error: new RpcError(-32000, 'ask failed') })).toBeNull()
  expect(awaitsRestart(false, { isSuccess: false, error: new Error('NOT_RUNNING') })).toBeNull()
})
