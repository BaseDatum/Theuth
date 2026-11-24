import './test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type { AdvisorDefaults } from "@/rpc/bindings/method.agent.output"
import type { ModelInfo } from "@/rpc/bindings/method.agent.output"
import type { SyncBacklog } from "@/rpc/bindings/method.agent.output"
import type * as Conn from '@/store/connection'
import { useConnection } from '@/store/connection'
import type { StoreApi, UseBoundStore } from 'zustand'
import { Advisor, BACKLOGS, CONSULTS, IMMUNE_RANGE, NOTES_RANGE, asBacklog, asConsults, backlogLabel, consultOptions, consultsLabel } from './Advisor'
import { bounded } from './shared'

const sonnet = { id: 'anthropic/claude-sonnet-4-5', name: 'Claude Sonnet 4.5', provider: 'anthropic', efforts: ['low', 'high'] } as ModelInfo
const opus = { id: 'anthropic/claude-opus-4-8', name: 'Claude Opus 4.8', provider: 'anthropic', efforts: ['high'] } as ModelInfo

// The engine's live `Info.advisor` (simulated server state the setters change), and a reactive connection store
// holding the client's copy: `refreshInfo` re-reads `api.info()` into it, as in the app.
let advisor: AdvisorDefaults = { model: null, effort: null, max_notes_per_update: 4, immune_turns: 3, sync_backlog: 'off', max_consults: 6 }
type ConnState = { info: { advisor: AdvisorDefaults } | null; api: null; discovery: { phase: 'legacy' }; setInfo: (info: { advisor: AdvisorDefaults }) => void }
vi.mock('@/store/connection', async (original) => {
  // A vi.mock factory is hoisted above the file's imports, so zustand is loaded here.
  const { create } = await import('zustand')
  return { ...(await original<typeof Conn>()), useConnection: create<ConnState>()((set) => ({ info: null, api: null, discovery: { phase: 'legacy' }, setInfo: (info) => set({ info }) })) }
})
const conn = useConnection as unknown as UseBoundStore<StoreApi<ConnState>>

const api = {
  list_models: vi.fn(async () => [sonnet, opus]),
  info: vi.fn(async () => ({ advisor })),
  // The engine's D4c/d: a model that does not take the stored effort drops it; None clears it.
  set_advisor_default_model: vi.fn(async (model: string | null) => {
    const keeps = [sonnet, opus].find((m) => m.id === model)?.efforts.includes(advisor.effort ?? '')
    return (advisor = { ...advisor, model, effort: keeps ? advisor.effort : null })
  }),
  set_advisor_default_effort: vi.fn(async (effort: string | null) => (advisor = { ...advisor, effort })),
}
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => api,
}))

test('notes per update accepts 1–32, immune turns 0–10; anything else leaves the field alone', () => {
  expect(bounded('1', ...NOTES_RANGE)).toBe(1)
  expect(bounded('32', ...NOTES_RANGE)).toBe(32)
  expect(bounded('4.9', ...NOTES_RANGE)).toBe(4)
  expect(bounded('0', ...NOTES_RANGE)).toBeUndefined()
  expect(bounded('33', ...NOTES_RANGE)).toBeUndefined()
  expect(bounded('abc', ...NOTES_RANGE)).toBeUndefined()
  expect(bounded('0', ...IMMUNE_RANGE)).toBe(0)
  expect(bounded('10', ...IMMUNE_RANGE)).toBe(10)
  expect(bounded('11', ...IMMUNE_RANGE)).toBeUndefined()
  expect(bounded('-1', ...IMMUNE_RANGE)).toBeUndefined()
})

test('SyncBacklog round-trips through the Catch-up select', () => {
  const all: SyncBacklog[] = ['off', '1', '3', '5']
  expect([...BACKLOGS]).toEqual(all)
  for (const v of all) expect(asBacklog(v)).toBe(v)
  expect(asBacklog('7')).toBeUndefined()
  expect(asBacklog('')).toBeUndefined()
  expect(backlogLabel('off')).toBe('Off')
  expect(backlogLabel('3')).toBe('3')
})

test('R1: Consults per turn offers Off · 2 · 6 · 12 (plus an off-list live value), labelled, and parses back to the cap', () => {
  expect([...CONSULTS]).toEqual([0, 2, 6, 12])
  expect(CONSULTS.map(consultsLabel)).toEqual(['Off', '2', '6', '12'])
  expect(consultOptions(6)).toEqual([0, 2, 6, 12])
  expect(consultOptions(undefined)).toEqual([0, 2, 6, 12])
  expect(consultOptions(32)).toEqual([0, 2, 6, 12, 32])
  for (const v of CONSULTS) expect(asConsults(String(v))).toBe(v)
  expect(asConsults('32')).toBe(32)
  expect(asConsults('33')).toBeUndefined()
  expect(asConsults('-1')).toBeUndefined()
  expect(asConsults('')).toBeUndefined()
})

// ── 194 row 6: "Default advisor model" is the standard picker over `advisor.model`, `advisor.effort` its chips ──────

window.HTMLElement.prototype.scrollIntoView = () => {}
afterEach(() => {
  document.body.innerHTML = ''
})

/** Mount Settings › Advisor for real and wait for the picker's trigger to read `ready`. */
async function mount(ready: string): Promise<void> {
  conn.setState({ info: { advisor } })
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const host = document.createElement('div')
  document.body.append(host)
  await act(async () => createRoot(host).render(createElement(QueryClientProvider, { client: qc, children: createElement(Advisor) as ReactNode })))
  await vi.waitFor(() => expect(trigger().textContent).toContain(ready))
}
const trigger = () => document.querySelector<HTMLElement>('button[aria-label="Default advisor model"]')!
const open = () => act(async () => void trigger().click())

test('194: a default advisor model with efforts shows its effort and chips; a chip calls set_advisor_default_effort', async () => {
  advisor = { ...advisor, model: sonnet.id, effort: 'high' }
  // 215 C4: the quiet trigger's effort is its own span beside the name (no space in textContent).
  await mount('Claude Sonnet 4.5· high')
  await open()
  const chips = [...document.querySelectorAll<HTMLElement>('[aria-label="Effort"] > *')]
  expect(chips.map((c) => c.textContent)).toEqual(['Default', 'low', 'high'])
  await act(async () => void chips[1]!.click())
  await vi.waitFor(() => expect(api.set_advisor_default_effort).toHaveBeenCalledWith('low'))
})

test('194: the None row shows no chips', async () => {
  advisor = { ...advisor, model: null, effort: null }
  await mount('None')
  await open()
  await vi.waitFor(() => expect(document.querySelector('[cmdk-item]')).not.toBeNull())
  expect(document.querySelector('[aria-label="Effort"]')).toBeNull()
})

test('194: a model pick that drops the effort shows it: the trigger loses `· effort`, the Default chip is selected', async () => {
  advisor = { ...advisor, model: sonnet.id, effort: 'low' }
  await mount('Claude Sonnet 4.5· low')
  await open()
  // The selected chip carries the check mark (under a favorite/provider row the chips are cmdk items, no `aria-pressed`).
  const pressed = () => [...document.querySelectorAll<HTMLElement>('[aria-label="Effort"] > *')].filter((c) => c.querySelector('svg')).map((c) => c.textContent)
  expect(pressed()).toEqual(['low'])
  const input = document.querySelector<HTMLInputElement>('[cmdk-input]')!
  await act(async () => {
    Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')!.set!.call(input, 'opus')
    input.dispatchEvent(new Event('input', { bubbles: true }))
  })
  await act(async () => void input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })))
  expect(api.set_advisor_default_model).toHaveBeenLastCalledWith(opus.id)
  // `opus` does not take `low`: the engine dropped it, and the refetched Info shows that.
  await vi.waitFor(() => expect(trigger().textContent).toBe('Claude Opus 4.8'))
  // 2026-11-17: opus has levels, so the picker stayed open and shows its chips once the pick lands.
  await vi.waitFor(() => expect([...document.querySelectorAll<HTMLElement>('[aria-label="Effort"] > *')].map((c) => c.textContent)).toEqual(['Default', 'high']))
  expect(pressed()).toEqual(['Default'])
})
