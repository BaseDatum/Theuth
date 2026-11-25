import { expect, test } from 'vitest'
import type { Api } from '@/rpc/bindings/client'
import { setAlwaysAskLevel } from './always-ask'

const CHAT = '0190a5b2-0000-7000-8000-000000000001'

/** Records each `set_always_ask` call; any removal fails the way a denied human-only card does. */
const engine = () => {
  const calls: [string | null, string, boolean][] = []
  const api = {
    set_always_ask: async (session: string | null, tool: string, on: boolean) => {
      calls.push([session, tool, on])
      if (!on) throw new Error(`always_ask: removing ${tool} was not approved`)
      return { global: [], session: [], inherited: [] }
    },
  } as unknown as Api
  return { api, calls }
}

test('219 §6.1: always → this chat adds the chat entry before removing the harness-wide one; a refused removal keeps the tool marked', async () => {
  const { api, calls } = engine()
  await expect(setAlwaysAskLevel(api, CHAT, { global: ['shell'], session: [], inherited: [] }, 'shell', 'session')).rejects.toThrow('not approved')
  expect(calls).toEqual([
    [CHAT, 'shell', true],
    [null, 'shell', false],
  ])
})

test('219 §6.1: this chat → always adds the harness-wide entry and keeps the chat one', async () => {
  const { api, calls } = engine()
  await setAlwaysAskLevel(api, CHAT, { global: [], session: ['shell'], inherited: [] }, 'shell', 'always')
  expect(calls).toEqual([[null, 'shell', true]])
})
