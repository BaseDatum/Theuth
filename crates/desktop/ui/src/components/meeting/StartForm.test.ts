import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import { StartForm, startOptions } from './StartForm'

vi.mock('@tanstack/react-router', async (original) => ({ ...(await original<object>()), useNavigate: () => () => {} }))
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({
    meeting_processes: async () => [],
    meeting_permissions: async () => ({ microphone: 'Authorized', audio_capture: 'Authorized' }),
    list_models: async () => [],
    endless_settings_get: async () => ({ default_meeting: true, model: null }),
  }),
}))

async function render(): Promise<string> {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const paint = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(StartForm) as ReactNode }))
  paint()
  for (const query of qc.getQueryCache().findAll()) await qc.fetchQuery({ ...query.options, queryKey: query.queryKey })
  return paint()
}

test('the ∞ switch follows agent.endless.default_meeting and sits under the model row with its one-line note', async () => {
  const html = await render()
  expect(html).toMatch(/Session model.*aria-checked="true"[^>]*aria-label="Endless memory"/s)
  expect(html).toContain('Keeps a compressed memory of the whole meeting behind the live transcript; zoom and recall bring any earlier moment back.')
})

test('startOptions sends memory only with the switch on; the rest is unchanged', () => {
  expect(startOptions({ pid: 0, title: ' Standup ', context: '', consent: true, model: 'm', endless: true })).toEqual({
    process: null,
    title: 'Standup',
    context: null,
    consent: true,
    session_model: 'm',
    memory: 'endless',
  })
  expect(startOptions({ pid: 42, title: '', context: 'x', consent: false, model: null, endless: false })).toMatchObject({ process: 42, memory: null })
})
