import './test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement } from 'react'
import { createRoot } from 'react-dom/client'
import { expect, test, vi } from 'vitest'
import { queryKeys } from '@/lib/rpc'
import type * as Rpc from '@/lib/rpc'
import { dispatch } from '@/store/sessions'
import { ModelProviders } from './ModelProviders'

const api = vi.hoisted(() => ({ login: vi.fn(async () => 'https://auth.example/authorize?x=1') }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => api }))

test('Log in on a local engine asks the engine once (it opens the browser) and only shows the URL as a fallback until LoginFinished', async () => {
  const open = vi.spyOn(window, 'open').mockImplementation(() => null)
  const client = new QueryClient()
  client.setQueryData(queryKeys.providers, [])
  client.setQueryData(queryKeys.logins, [{ name: 'anthropic', logged_in: false, account: null, expires_at: null }])
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  await act(async () => root.render(createElement(QueryClientProvider, { client }, createElement(ModelProviders))))
  const button = [...host.querySelectorAll('button')].find((b) => b.textContent === 'Log in')!
  await act(async () => button.click())
  expect(api.login).toHaveBeenCalledTimes(1)
  expect(api.login).toHaveBeenCalledWith('anthropic')
  expect(open).not.toHaveBeenCalled()
  expect(host.textContent).toContain("Browser didn't open? https://auth.example/authorize?x=1")
  await act(async () => dispatch([{ key: 'agent.login@1', event: { type: 'LoginFinished', provider: 'anthropic', error: null } }]))
  expect(host.textContent).not.toContain('https://auth.example')
  await act(async () => root.unmount())
})
