import { expect, test, vi } from 'vitest'
import { forEngine, namedParams } from '@/lib/engine-call'
import type { Api } from '@/rpc/bindings/client'

function fakeApi() {
  return {
    set_model: vi.fn(async () => null),
    info: vi.fn(async () => ({ name: 'here' })),
    engine_call: vi.fn(async (_engine: string, method: string) => (method === 'info' ? { name: 'there' } : null)),
  } as unknown as Api & { engine_call: ReturnType<typeof vi.fn>; set_model: ReturnType<typeof vi.fn> }
}

test('227: the connected engine is the api itself: no engine_call', async () => {
  const api = fakeApi()
  const here = forEngine(api, 'mac', 'mac')
  expect(here).toBe(api)
  await here.set_model(null, 'm', true)
  expect(api.set_model).toHaveBeenCalledWith(null, 'm', true)
  expect(api.engine_call).not.toHaveBeenCalled()
})

test('227: another engine: m(...args) becomes engine_call(target, m, named params), with the bindings\' names and no press', async () => {
  const api = fakeApi()
  const there = forEngine(api, 'p50', 'mac')
  await there.set_model(null, 'openai/gpt-6', true)
  expect(api.engine_call).toHaveBeenLastCalledWith('p50', 'set_model', { session: null, model: 'openai/gpt-6', manual: true })
  await there.config_flag_set('knowledge.peek', false)
  expect(api.engine_call).toHaveBeenLastCalledWith('p50', 'config_flag_set', { key: 'knowledge.peek', on: false })
  // A reply comes back as the method's own would.
  await expect(there.info()).resolves.toMatchObject({ name: 'there' })
  expect(api.set_model).not.toHaveBeenCalled()
})

test('227: a vault method is not forwarded: it runs here with the picked engine as its own `engine` argument', async () => {
  const vault_status = vi.fn(async () => ({ sealed: false }))
  const api = { ...fakeApi(), vault_status } as unknown as Api & { engine_call: ReturnType<typeof vi.fn> }
  const there = forEngine(api, 'p50', 'mac') as unknown as { vault_status(engine: string | null): Promise<unknown> }
  await there.vault_status(null)
  expect(vault_status).toHaveBeenLastCalledWith('p50')
  await there.vault_status('nyc')
  expect(vault_status).toHaveBeenLastCalledWith('nyc')
  expect(api.engine_call).not.toHaveBeenCalled()
})

test('227: omitted trailing optionals are left out; too many arguments or an unknown method throw', () => {
  expect(namedParams('set_builtins_disabled', [['a'], true, undefined])).toEqual({ names: ['a'], disabled: true })
  expect(() => namedParams('config_flag_set', ['k', true, 'extra'])).toThrow(/takes 2 parameters/)
  expect(() => namedParams('no_such_method', [])).toThrow(/not a known method/)
  // The 227 methods themselves are known before the bindings carry them.
  expect(namedParams('engine_caps', [])).toEqual({})
})
