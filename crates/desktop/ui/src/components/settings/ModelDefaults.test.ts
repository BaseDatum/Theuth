import './test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type { ModelInfo } from "@/rpc/bindings/method.agent.output"
import type { Info } from "@/rpc/bindings/method.rpc_2dcompose.output"
import type * as Conn from '@/store/connection'
import { ModelDefaults } from './ModelDefaults'

// `renderToStaticMarkup` reads zustand's initial state, so the engine `info` is a plain pick here.
let info: Partial<Info> = {}
vi.mock('@/store/connection', async (original) => ({
  ...(await original<typeof Conn>()),
  useConnection: (pick: (s: unknown) => unknown) => pick({ info, api: null , discovery: { phase: 'legacy' } }),
}))

const sonnet = { id: 'anthropic/claude-sonnet-4-5', name: 'Claude Sonnet 4.5', provider: 'anthropic', efforts: ['low', 'high'] } as ModelInfo
const haiku = { id: 'anthropic/claude-haiku-4-5', name: 'Claude Haiku 4.5', provider: 'anthropic', efforts: [] as string[] } as ModelInfo
const opus = { id: 'anthropic/claude-opus-4-8', name: 'Claude Opus 4.8', provider: 'anthropic', efforts: ['high'] } as ModelInfo

const api = {
  list_models: vi.fn(async () => [sonnet, haiku, opus]),
  subagent_model: vi.fn(async (): Promise<ModelInfo | null> => haiku),
  subagent_effort: vi.fn(async (): Promise<string | null> => null),
  theuth_agent_model: vi.fn(async (): Promise<ModelInfo | null> => null),
  theuth_agent_effort: vi.fn(async (): Promise<string | null> => null),
  set_model: vi.fn(),
  set_effort: vi.fn(),
  set_subagent_model: vi.fn(async (id: string | null) => [sonnet, haiku, opus].find((m) => m.id === id) ?? null),
  set_subagent_effort: vi.fn(async (level: string | null) => level),
  set_theuth_agent_model: vi.fn(),
  set_theuth_agent_effort: vi.fn(async (level: string | null) => level),
}

vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => api,
}))

async function render(): Promise<string> {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const paint = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(ModelDefaults) as ReactNode }))
  paint()
  for (const query of qc.getQueryCache().findAll()) await qc.fetchQuery({ ...query.options, queryKey: query.queryKey })
  return paint()
}

test('148: new sessions shows the live default with its effort; subagents reads subagent_model; theuth reads theuth_agent_model with "Same as new sessions" when unset', async () => {
  info = { default_model: sonnet.id, default_effort: 'high' }
  const html = await render()
  expect(html).toMatch(/aria-label="New sessions model"[^>]*>.*?Claude Sonnet 4\.5<\/span><span class="shrink-0">· high<\/span>/s)
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
  expect(html).not.toContain('Claude Haiku 4.5</span><span class="shrink-0">· high')
})

// ── 194: the Subagents and theuth agent rows carry their effort chips ──────────────────────────────────────────────

window.HTMLElement.prototype.scrollIntoView = () => {}
afterEach(() => {
  document.body.innerHTML = ''
})

/** Mount the section for real (the picker opens in a portal); wait until `label`'s trigger reads `ready` and its queries settled (enabled). */
async function mount(label: string, ready: string): Promise<void> {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const host = document.createElement('div')
  document.body.append(host)
  await act(async () => createRoot(host).render(createElement(QueryClientProvider, { client: qc, children: createElement(ModelDefaults) as ReactNode })))
  await vi.waitFor(() => expect([trigger(label).textContent?.includes(ready), trigger(label).disabled]).toEqual([true, false]))
}
const trigger = (label: string) => document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!
const open = (label: string) => act(async () => void trigger(label).click())
const chips = () => [...document.querySelectorAll<HTMLElement>('[aria-label="Effort"] > *')]

test('194: a subagent model with efforts shows its stored effort and chips; a chip calls set_subagent_effort and refetches it', async () => {
  info = { default_model: sonnet.id }
  api.subagent_model.mockImplementation(async () => sonnet)
  api.subagent_effort.mockImplementation(async () => 'high')
  // 215 C4: the quiet trigger's effort is its own span beside the name (no space in textContent).
  await mount('Subagent model', 'Claude Sonnet 4.5· high')
  await open('Subagent model')
  expect(chips().map((c) => c.textContent)).toEqual(['Default', 'low', 'high'])
  const reads = api.subagent_effort.mock.calls.length
  await act(async () => void chips()[1]!.click())
  await vi.waitFor(() => expect(api.subagent_effort.mock.calls.length).toBeGreaterThan(reads))
  expect(api.set_subagent_effort).toHaveBeenLastCalledWith('low')
})

test('194: the "Same as new sessions" row shows no chips (theuth agent unset)', async () => {
  await mount('theuth agent model', 'Same as new sessions')
  await open('theuth agent model')
  await vi.waitFor(() => expect(document.querySelector('[cmdk-item]')).not.toBeNull())
  expect(document.querySelector('[aria-label="Effort"]')).toBeNull()
})

test('194: picking another subagent model refetches the subagent effort (the engine may have dropped it)', async () => {
  api.subagent_model.mockImplementation(async () => sonnet)
  await mount('Subagent model', 'Claude Sonnet 4.5')
  const reads = api.subagent_effort.mock.calls.length
  await open('Subagent model')
  const input = document.querySelector<HTMLInputElement>('[cmdk-input]')!
  await act(async () => {
    Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')!.set!.call(input, 'opus')
    input.dispatchEvent(new Event('input', { bubbles: true }))
  })
  await act(async () => void input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })))
  await vi.waitFor(() => expect(api.subagent_effort.mock.calls.length).toBeGreaterThan(reads))
  expect(api.set_subagent_model).toHaveBeenLastCalledWith(opus.id)
})

test('2026-11-17: picking a subagent model with levels sets it once and keeps the picker open; its chips show, focused, once it lands; a chip sets the effort and closes', async () => {
  api.subagent_model.mockImplementation(async () => haiku)
  api.subagent_effort.mockImplementation(async () => null)
  api.set_subagent_model.mockImplementation(async (id) => {
    const m = [sonnet, haiku, opus].find((x) => x.id === id) ?? null
    api.subagent_model.mockImplementation(async () => m)
    return m
  })
  api.set_subagent_model.mockClear()
  api.set_subagent_effort.mockClear()
  await mount('Subagent model', 'Claude Haiku 4.5')
  await open('Subagent model')
  await act(async () => void document.querySelector<HTMLElement>(`[cmdk-item][data-value="model:${sonnet.id}"]`)!.click())
  // The pick round-trips (set_subagent_model, then the refetch); the chips wait for it, then take the focus.
  await vi.waitFor(() => expect(document.activeElement?.textContent).toBe('Default'))
  expect(chips().map((c) => c.textContent)).toEqual(['Default', 'low', 'high'])
  expect(api.set_subagent_model.mock.calls).toEqual([[sonnet.id]])
  expect(api.set_subagent_effort).not.toHaveBeenCalled()
  await act(async () => void chips()[2]!.click())
  expect(document.querySelector('[cmdk-root]')).toBeNull()
  expect(api.set_subagent_effort.mock.calls).toEqual([['high']])
  expect(api.set_subagent_model).toHaveBeenCalledTimes(1)
})
