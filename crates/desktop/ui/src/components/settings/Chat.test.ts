import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type { ModelInfo } from '@/rpc/bindings/ModelInfo'
import type { Info } from '@/rpc/extra'
import type * as Conn from '@/store/connection'
import { Chat } from './Chat'

// `renderToStaticMarkup` reads zustand's initial state, so the engine `info` is a plain pick here.
let info: Partial<Info> = {}
vi.mock('@/store/connection', async (original) => ({
  ...(await original<typeof Conn>()),
  useConnection: (pick: (s: unknown) => unknown) => pick({ info, api: null }),
}))

const sonnet = { id: 'anthropic/claude-sonnet-4-5', name: 'Claude Sonnet 4.5', efforts: ['low', 'high'] } as ModelInfo
const haiku = { id: 'anthropic/claude-haiku-4-5', name: 'Claude Haiku 4.5', efforts: [] as string[] } as ModelInfo
const opus = { id: 'anthropic/claude-opus-4-8', name: 'Claude Opus 4.8', efforts: ['high'] } as ModelInfo

const api = {
  list_models: vi.fn(async () => [sonnet, haiku, opus]),
  subagent_model: vi.fn(async () => haiku),
  theuth_agent_model: vi.fn(async (): Promise<ModelInfo | null> => null),
  set_model: vi.fn(),
  set_effort: vi.fn(),
  set_subagent_model: vi.fn(),
  set_theuth_agent_model: vi.fn(),
}

vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => api,
}))

async function render(): Promise<string> {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const paint = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(Chat) as ReactNode }))
  paint()
  for (const query of qc.getQueryCache().findAll()) await qc.fetchQuery({ ...query.options, queryKey: query.queryKey })
  return paint()
}

test('148: the Models section leads — new sessions shows the live default with its effort; subagents reads subagent_model; theuth reads theuth_agent_model with "Same as new sessions" when unset', async () => {
  info = { default_model: sonnet.id, default_effort: 'high' }
  const html = await render()
  expect(html.indexOf('Models')).toBeLessThan(html.indexOf('Tool calls'))
  expect(html).toMatch(/aria-label="New sessions model"[^>]*>.*?Claude Sonnet 4\.5 · high<\/span>/s)
  expect(html).toContain('Picking a model inside a chat changes only that chat.')
  expect(html).toMatch(/aria-label="Subagent model"[^>]*>.*?Claude Haiku 4\.5<\/span>/s)
  expect(html).toContain('External harnesses (omp, OpenCode) keep their own model.')
  expect(html).toMatch(/aria-label="theuth agent model"[^>]*>.*?Same as new sessions<\/span>/s)
  expect(html).toContain('its desk and the work it starts for tasks.')
  expect(api.subagent_model).toHaveBeenCalled()
  expect(api.theuth_agent_model).toHaveBeenCalled()
  // The default row has no "none": the engine always has a default model.
  expect(html.match(/Same as new sessions/g)).toHaveLength(1)
})

test('148: the new-sessions row follows DefaultModelChanged (the Info patch), and a model without effort levels shows no effort', async () => {
  info = { default_model: haiku.id, default_effort: 'high' }
  const html = await render()
  expect(html).toMatch(/aria-label="New sessions model"[^>]*>.*?Claude Haiku 4\.5<\/span>/s)
  expect(html).not.toContain('Claude Haiku 4.5 · high')
})
