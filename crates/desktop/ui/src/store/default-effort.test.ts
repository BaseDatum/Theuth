import { expect, test } from 'vitest'
import type { Info } from "@/rpc/bindings/method.rpc_2dcompose.output"
import { scope } from './engine-scope'
import { dispatch } from './sessions'

// 83 §3: the new-chat draft reads `info.default_effort`; `DefaultEffortChanged` keeps it live
// for the engine that published it, whether or not that engine is the active one.
test('DefaultEffortChanged updates the publishing engine Info only', () => {
  const a = scope('test-default-effort-a')
  const b = scope('test-default-effort-b')
  const info = { default_model: 'anthropic/claude-sonnet-4-5', default_effort: 'medium' } as Info
  a.connection.getState().setInfo(info)
  b.connection.getState().setInfo(info)
  dispatch([{ type: 'DefaultEffortChanged', effort: 'high' }], a)
  expect(a.connection.getState().info?.default_effort).toBe('high')
  expect(b.connection.getState().info?.default_effort).toBe('medium')
  // A clear is `null`, not a missing field.
  dispatch([{ type: 'DefaultEffortChanged', effort: null }], a)
  expect(a.connection.getState().info?.default_effort).toBeNull()
  // Before `info()` answered there is nothing to patch; the reply itself carries the live value.
  b.connection.getState().setInfo(null)
  dispatch([{ type: 'DefaultEffortChanged', effort: 'high' }], b)
  expect(b.connection.getState().info).toBeNull()
})
