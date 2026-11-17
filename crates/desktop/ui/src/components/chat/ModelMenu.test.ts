import '../settings/test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement, type ReactElement, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, expect, type Mock, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type { ConfigOption } from "@/rpc/bindings/method.agent.output"
import type { ModelInfo } from "@/rpc/bindings/method.agent.output"
import { useConnection } from '@/store/connection'
import { useSessions } from '@/store/sessions'
import { AdvisorCommand, dialogPlacement, HarnessPicker, harnessCurrent, harnessGroups, type Model, ModelMenu, slashTarget } from './ModelMenu'
import { useDrafts } from '@/lib/drafts'

const rpc = vi.hoisted(() => ({ api: {} as Record<string, unknown> }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => rpc.api }))

const advisor = vi.hoisted(() => ({ status: undefined as unknown, mutate: (() => {}) as (v: unknown) => void }))
vi.mock('./advisor', () => ({ useAdvisor: () => advisor.status, useSetAdvisor: () => ({ mutate: (v: unknown) => advisor.mutate(v) }) }))

window.HTMLElement.prototype.scrollIntoView = () => {}
afterEach(() => {
  document.body.innerHTML = ''
})

const opt = (id: string, category: string | null, current: string, values: [string, string][]): ConfigOption => ({
  id,
  name: id,
  category,
  current,
  options: values.map(([value, name]) => ({ value, name, description: null })),
})

test('harness picker groups the model select and the thought_level option; a queued change shows as pending', () => {
  // omp: mode, model, thinking; opencode without variants: model, mode only.
  const omp = [
    opt('mode', 'mode', 'default', [['default', 'Default']]),
    opt('model', 'model', 'anthropic/claude-opus-4-8', [['anthropic/claude-opus-4-8', 'Claude Opus 4.8'], ['openai/gpt-5.4', 'GPT-5.4']]),
    opt('thinking', 'thought_level', 'high', [['off', 'Off'], ['high', 'High'], ['xhigh', 'Extra high']]),
  ]
  const g = harnessGroups(omp)
  expect(g.model?.id).toBe('model')
  expect(g.effort?.id).toBe('thinking')
  expect(harnessGroups(omp.slice(0, 2)).effort).toBeUndefined()
  // No category: the id/name fallback finds the model select.
  expect(harnessGroups([opt('llm', null, 'x', [['x', 'X']]), opt('Active model', null, 'y', [['y', 'Y']])]).model?.id).toBe('Active model')

  expect(harnessCurrent(g.model, undefined)).toBe('Claude Opus 4.8')
  expect(harnessCurrent(g.model, 'openai/gpt-5.4')).toBe('→ openai/gpt-5.4 (after this turn)')
  expect(harnessCurrent(opt('m', 'model', 'raw/id', []), undefined)).toBe('raw/id')
  expect(harnessCurrent(undefined, undefined)).toBeUndefined()
})

async function mount(el: ReactElement) {
  const host = document.createElement('div')
  document.body.append(host)
  await act(async () => createRoot(host).render(el))
  const lead = (text: string) => [...host.querySelectorAll('button')].find((b) => b.textContent === text)
  /** The chip row right after `row`: labels, the active one starred. */
  const chipsAfter = (row: Element | null | undefined) => {
    const g = row?.nextElementSibling
    return g?.getAttribute('aria-label') === 'Effort' ? [...g.children].map((c) => `${c.textContent}${c.querySelector('svg') ? '*' : ''}`) : null
  }
  const chip = (label: string) => [...host.querySelectorAll<HTMLElement>('[aria-label="Effort"] > *')].find((c) => c.textContent === label)!
  const click = (el: Element) => act(async () => void (el as HTMLElement).click())
  return { host, lead, chipsAfter, chip, click }
}

const omp = [
  opt('mode', 'mode', 'default', [['default', 'Default']]),
  opt('model', 'model', 'anthropic/claude-opus-4-8', [['anthropic/claude-opus-4-8', 'Claude Opus 4.8'], ['openai/gpt-5.4', 'GPT-5.4']]),
  opt('thinking', 'thought_level', 'high', [['off', 'Off'], ['high', 'High'], ['xhigh', 'Extra high']]),
]
const harnessM = (harness: ConfigOption[], pending: Record<string, string> = {}) => ({ agent: 'omp', harnessModel: undefined, harness, pending, setHarnessOption: vi.fn(async () => {}), done: vi.fn() }) as unknown as Model & { setHarnessOption: Mock; done: Mock }

test('the ACP harness body is the standard picker: its model options as rows by provider, its thought_level as chips under the current one (no Default)', async () => {
  const m = harnessM(omp)
  const p = await mount(createElement(HarnessPicker, { m }))
  expect(p.chipsAfter(p.lead('Claude Opus 4.8anthropic'))).toEqual(['Off', 'High*', 'Extra high'])
  expect([...p.host.querySelectorAll('[cmdk-item]')].map((e) => e.getAttribute('data-value'))).toEqual(['provider:anthropic', 'provider:openai'])
  await p.click(p.chip('Extra high'))
  expect(m.setHarnessOption).toHaveBeenCalledWith('thinking', 'xhigh')
  await p.click(p.host.querySelector('[data-value="provider:openai"]')!)
  await p.click(p.host.querySelector('[data-value="model:openai/gpt-5.4"]')!)
  expect(m.setHarnessOption).toHaveBeenLastCalledWith('model', 'openai/gpt-5.4')
})

test('harness picker: a queued value reads "after this turn"; no model select → one row for what it runs, still with chips; no thought_level → no chips', async () => {
  const queued = await mount(createElement(HarnessPicker, { m: harnessM(omp, { thinking: 'off' }) }))
  expect(queued.chipsAfter(queued.lead('Claude Opus 4.8anthropic'))).toEqual(['Off · after this turn', 'High*', 'Extra high'])
  const only = await mount(createElement(HarnessPicker, { m: { ...harnessM([omp[2]!]), harnessModel: 'claude-x' } }))
  expect(only.chipsAfter(only.host.querySelector('[data-value="model:omp"]'))).toEqual(['Off', 'High*', 'Extra high'])
  const plain = await mount(createElement(HarnessPicker, { m: harnessM(omp.slice(0, 2)) }))
  expect(plain.host.querySelector('[aria-label="Effort"]')).toBeNull()
})

const catalog: ModelInfo[] = [
  { id: 'openai/gpt-5.4', name: 'GPT-5.4', provider: 'openai', context: 0, reasoning: true, image: false, efforts: ['low', 'high'], budget_min: null, budget_max: null, local: false },
  { id: 'anthropic/claude-opus-4-1', name: 'Claude Opus 4.1', provider: 'anthropic', context: 0, reasoning: false, image: false, efforts: [], budget_min: null, budget_max: null, local: false },
]

test('125 §8.1 the advisor picker passes the advisor model’s levels: chips under it, a chip re-picks the same model with that effort and closes', async () => {
  advisor.status = { state: 'on', model: 'openai/gpt-5.4', effort: 'low' }
  advisor.mutate = vi.fn()
  const m = { models: catalog, favorites: [], toggleFavorite: async () => {}, done: vi.fn() } as unknown as Model & { done: Mock }
  const p = await mount(createElement(AdvisorCommand, { session: 's1', m, onBack() {} }))
  expect(p.chipsAfter(p.lead('GPT-5.4openai'))).toEqual(['Default', 'low*', 'high'])
  await p.click(p.chip('high'))
  expect(advisor.mutate).toHaveBeenCalledWith({ model: 'openai/gpt-5.4', effort: 'high' })
  expect(m.done).toHaveBeenCalled()
  // Off (no advisor model): the Off row is selected and nothing has levels to show.
  advisor.status = { state: 'off', model: null, effort: null }
  const off = await mount(createElement(AdvisorCommand, { session: 's1', m, onBack() {} }))
  expect(off.host.querySelector('[aria-label="Effort"]')).toBeNull()
})

test('127 §6 dialogPlacement: touch anchors the dialog to the top of the visible band and lets the list fill it; else centred', () => {
  expect(dialogPlacement(false)).toEqual({ content: '', list: '', picker: '', footer: '' })
  const phone = dialogPlacement(true)
  expect(phone.content.split(' ')).toEqual(expect.arrayContaining(['top-[max(0.5rem,env(safe-area-inset-top))]', 'translate-y-0', 'flex', 'flex-col', 'max-h-[calc(var(--visible-band)-1rem)]']))
  expect(phone.content).not.toMatch(/translate-x|left-/)
  expect(phone.list.split(' ')).toEqual(expect.arrayContaining(['max-h-none', 'min-h-0', 'flex-1']))
  // 125 F2: the Command root stops claiming the whole band (`h-full`) so the Advisor row stays a visible footer.
  expect(phone.picker.split(' ')).toEqual(expect.arrayContaining(['h-auto', 'min-h-0', 'flex-1']))
  expect(phone.footer.split(' ')).toContain('shrink-0')
})

test('157 D1 slashTarget: /model reaches the focused pane, any draft, and a page-own picker whatever is focused', () => {
  expect(slashTarget('s1', 's1', false)).toBe(true)
  expect(slashTarget('s1', 's2', false)).toBe(false)
  expect(slashTarget('s1', null, false)).toBe(false)
  expect(slashTarget(null, 's2', false)).toBe(true)
  // The notebook's chat column: no split pane ever focuses its thread, so it answers on its own.
  expect(slashTarget('s1', 's2', true)).toBe(true)
  expect(slashTarget('s1', null, true)).toBe(true)
})

test('the draft advisor picker holds model and effort for creation instead of calling set_advisor', async () => {
  useDrafts.setState({ settings: { create: { advisor: ['openai/gpt-5.4', 'low'] } } })
  advisor.mutate = vi.fn()
  const m = { models: catalog, favorites: [], toggleFavorite: async () => {}, done: vi.fn() } as unknown as Model
  const p = await mount(createElement(AdvisorCommand, { session: '', m, onBack() {} }))
  expect(p.chipsAfter(p.lead('GPT-5.4openai'))).toEqual(['Default', 'low*', 'high'])
  await p.click(p.chip('high'))
  expect(useDrafts.getState().settings.create?.advisor).toEqual(['openai/gpt-5.4', 'high'])
  expect(advisor.mutate).not.toHaveBeenCalled()
  useDrafts.setState({ settings: {} })
})

// ── 2026-11-17 (Sky): picking a model with levels keeps the picker open on its chips, in every wrapper ───────────────

test('harness picker: a model pick keeps it open on the thought_level chips, focused, even queued for the turn’s end; a chip completes it', async () => {
  const setHarnessOption = vi.fn()
  const done = vi.fn()
  function Harness() {
    // set_harness_option answers `pending`: the model is queued, `current` stays.
    const [pending, setPending] = useState<Record<string, string>>({})
    const m = { ...harnessM(omp, pending), done, setHarnessOption: async (id: string, v: string) => (setHarnessOption(id, v), setPending((p) => ({ ...p, [id]: v }))) }
    return createElement(HarnessPicker, { m: m as unknown as Model })
  }
  const p = await mount(createElement(Harness))
  await p.click(p.host.querySelector('[data-value="provider:openai"]')!)
  await p.click(p.host.querySelector('[data-value="model:openai/gpt-5.4"]')!)
  expect(setHarnessOption.mock.calls).toEqual([['model', 'openai/gpt-5.4']])
  expect(done).not.toHaveBeenCalled()
  expect(p.chipsAfter(p.host.querySelector('[data-value="model:openai/gpt-5.4"]'))).toEqual(['Off', 'High*', 'Extra high'])
  expect(document.activeElement?.textContent).toBe('High')
  await p.click(p.chip('Off'))
  expect(setHarnessOption).toHaveBeenLastCalledWith('thinking', 'off')
  expect(done).toHaveBeenCalledTimes(1)
})

test('chat header (ModelMenu): a model with levels is set once and the popover stays on its chips, focused; Escape keeps it; a chip sets the effort once and closes; a model without levels closes at once', async () => {
  useConnection.setState({ discovery: { ...useConnection.getState().discovery, phase: 'legacy' }, info: { favorite_models: [] } as never })
  const slice = (patch: object) => useSessions.setState((s) => ({ sessions: { ...s.sessions, s1: { ...s.sessions.s1, ...patch } as never } }))
  slice({ model: 'anthropic/claude-opus-4-1', effort: null })
  const calls: unknown[][] = []
  rpc.api = {
    list_sessions: async () => [],
    list_models: async () => catalog,
    // The engine's ModelChanged / EffortChanged land in the session's slice.
    set_model: async (s: string, id: string) => (calls.push(['set_model', s, id]), slice({ model: id })),
    set_effort: async (s: string, level: string | null) => (calls.push(['set_effort', s, level]), slice({ effort: level })),
  }
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const p = await mount(createElement(QueryClientProvider, { client: qc }, createElement(ModelMenu, { session: 's1' })))
  const trigger = p.host.querySelector('button')!
  await vi.waitFor(() => expect(trigger.textContent).toBe('Claude Opus 4.1'))
  const isOpen = () => document.querySelector('[cmdk-root]') !== null
  const at = (sel: string) => document.querySelector<HTMLElement>(sel)!
  await p.click(trigger)
  await p.click(at('[data-value="provider:openai"]'))
  await p.click(at('[data-value="model:openai/gpt-5.4"]'))
  await vi.waitFor(() => expect(document.activeElement?.textContent).toBe('Default'))
  expect(calls).toEqual([['set_model', 's1', 'openai/gpt-5.4']])
  expect(isOpen()).toBe(true)
  await act(async () => void document.activeElement!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })))
  expect([isOpen(), calls.length, trigger.textContent]).toEqual([false, 1, 'GPT-5.4'])
  await p.click(trigger)
  await p.click(at('[aria-label="Effort"] [data-value="effort:high"]'))
  expect(calls).toEqual([['set_model', 's1', 'openai/gpt-5.4'], ['set_effort', 's1', 'high']])
  expect(isOpen()).toBe(false)
  await p.click(trigger)
  await p.click(at('[data-value="provider:anthropic"]'))
  await p.click(at('[data-value="model:anthropic/claude-opus-4-1"]'))
  expect(calls.at(-1)).toEqual(['set_model', 's1', 'anthropic/claude-opus-4-1'])
  expect(calls).toHaveLength(3)
  expect(isOpen()).toBe(false)
})
