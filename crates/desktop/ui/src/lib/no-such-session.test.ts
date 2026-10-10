import { expect, test } from 'vitest'
import { RpcError } from '@/rpc/client'
import { isNoSuchSession } from './no-such-session'

test('the engine\'s refusal, bare or id-prefixed, as the RpcError the client throws, an Error or a string', () => {
  expect(isNoSuchSession(new RpcError(-32000, 'no such session'))).toBe(true)
  expect(isNoSuchSession(new RpcError(-32000, '01a1272d-05ba-7763-8021-38c251094629: no such session'))).toBe(true)
  expect(isNoSuchSession(new Error('no such session'))).toBe(true)
  expect(isNoSuchSession('no such session')).toBe(true)
})

test('anything else is an error to show', () => {
  expect(isNoSuchSession(new RpcError(-32300, 'not connected'))).toBe(false)
  expect(isNoSuchSession(new RpcError(-32000, 'no such session: refused'))).toBe(false)
  expect(isNoSuchSession(new Error('session busy'))).toBe(false)
  expect(isNoSuchSession({ code: -32000, message: 'no such session' })).toBe(false)
  expect(isNoSuchSession(undefined)).toBe(false)
})
