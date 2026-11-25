import { expect, test, vi } from 'vitest'
import { pluginProvider } from './provider'

const api = () => ({ plugin_action: vi.fn(async () => ({ ok: true })), plugin_query: vi.fn(async () => ({ turns: 3 })) })
const bindings = [
  { statement_id: 's_reset', name: 'reset', kind: 'mutation' as const },
  { statement_id: 's_count', name: 'count', kind: 'query' as const },
]

test('a pressed mutation binding goes to plugin_action with the press id as activation', async () => {
  const a = api()
  const provider = pluginProvider(a, { plugin: 'turns', placement: 'turns', session: 's1', bindings })
  await expect(provider('ignored', { day: 'today' }, { statement_id: 's_reset', trigger: 'press', action_id: 'act-1' })).resolves.toEqual({ ok: true })
  expect(a.plugin_action).toHaveBeenCalledExactlyOnceWith('turns', 'turns', 'reset', { day: 'today' }, 's1', 'act-1', null)
  expect(a.plugin_query).not.toHaveBeenCalled()
})

test('a query binding goes to plugin_query on load and on a press alike', async () => {
  const a = api()
  const provider = pluginProvider(a, { plugin: 'turns', placement: 'turns', session: null, bindings })
  await provider('count', undefined, { statement_id: 's_count', trigger: 'load' })
  await provider('count', { a: 1 }, { statement_id: 's_count', trigger: 'press', action_id: 'act-2' })
  expect(a.plugin_query.mock.calls).toEqual([
    ['turns', 'turns', 'count', {}, null],
    ['turns', 'turns', 'count', { a: 1 }, null],
  ])
  expect(a.plugin_action).not.toHaveBeenCalled()
})

test('a mutation without a press and an unknown statement are refused before reaching the plugin', async () => {
  const a = api()
  const provider = pluginProvider(a, { plugin: 'turns', placement: 'turns', session: null, bindings })
  await expect(provider('reset', {}, { statement_id: 's_reset', trigger: 'load' })).rejects.toThrow('a mutation needs a press')
  await expect(provider('reset', {}, { statement_id: 'nope', trigger: 'press', action_id: 'x' })).rejects.toThrow('unknown statement nope')
  expect(a.plugin_action).not.toHaveBeenCalled()
  expect(a.plugin_query).not.toHaveBeenCalled()
})
