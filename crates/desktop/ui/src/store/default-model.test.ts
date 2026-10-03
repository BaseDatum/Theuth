import { expect, test } from 'vitest'
import type { Info } from '@/rpc/extra'
import { scope } from './engine-scope'
import { dispatch } from './sessions'

// 82 §3: the new-chat draft reads `info.default_model`; `DefaultModelChanged` keeps it live
// for the engine that published it, whether or not that engine is the active one.
test('DefaultModelChanged updates the publishing engine Info only', () => {
  const a = scope('test-default-model-a')
  const b = scope('test-default-model-b')
  const info = { default_model: 'anthropic/claude-sonnet-4-5' } as Info
  a.connection.getState().setInfo(info)
  b.connection.getState().setInfo(info)
  dispatch([{ type: 'DefaultModelChanged', model: 'openai/gpt-5.4' }], a)
  expect(a.connection.getState().info?.default_model).toBe('openai/gpt-5.4')
  expect(b.connection.getState().info?.default_model).toBe('anthropic/claude-sonnet-4-5')
  // Before `info()` answered there is nothing to patch; the reply itself carries the live value.
  b.connection.getState().setInfo(null)
  dispatch([{ type: 'DefaultModelChanged', model: 'openai/gpt-5.4' }], b)
  expect(b.connection.getState().info).toBeNull()
})
