import { expect, test } from 'vitest'
import type { Info } from "@/rpc/bindings/method.rpc_2dcompose.output"
import { scope } from './engine-scope'
import { dispatch } from './sessions'

// 127 §4: favorites belong to the engine; `FavoriteModelsChanged` replaces the publishing engine's list only.
test('FavoriteModelsChanged replaces the publishing engine Info list only', () => {
  const a = scope('test-favorite-models-a')
  const b = scope('test-favorite-models-b')
  const info = { default_model: 'anthropic/claude-sonnet-4-5', favorite_models: ['openai/gpt-5.4'] } as Info
  a.connection.getState().setInfo(info)
  b.connection.getState().setInfo(info)
  dispatch([{ type: 'FavoriteModelsChanged', models: ['anthropic/claude-sonnet-4-5', 'openai/gpt-5.4'] }], a)
  expect(a.connection.getState().info?.favorite_models).toEqual(['anthropic/claude-sonnet-4-5', 'openai/gpt-5.4'])
  expect(a.connection.getState().info?.default_model).toBe('anthropic/claude-sonnet-4-5')
  expect(b.connection.getState().info?.favorite_models).toEqual(['openai/gpt-5.4'])
  // Unstarring the last one leaves an empty list, not the old one.
  dispatch([{ type: 'FavoriteModelsChanged', models: [] }], a)
  expect(a.connection.getState().info?.favorite_models).toEqual([])
  // Before `info()` answered there is nothing to patch.
  b.connection.getState().setInfo(null)
  dispatch([{ type: 'FavoriteModelsChanged', models: ['x/y'] }], b)
  expect(b.connection.getState().info).toBeNull()
})
