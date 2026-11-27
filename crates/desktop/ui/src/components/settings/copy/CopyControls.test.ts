import '../test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement as h, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import { fromGetter, setterField } from '@/components/settings/copy/fields'
import { SetOnAllSheet, type SetOnAllRequest } from '@/components/settings/copy/SetOnAllSheet'
import { type SettingsMachines, SettingsMachinesProvider } from '@/components/settings/machine-target'
import { placeOf } from '@/components/settings/registry'
import { Row } from '@/components/settings/shared'
import type { FullApi } from '@/lib/engine-call.types'
import type { Machine } from '@/lib/mesh-machines'

const machine = (id: string, name: string, m: Partial<Machine> = {}): Machine => ({ id, name, kind: 'peer', connected: false, hub: false, online: true, sealed: false, path: 'direct', ...m })
const ipad = machine('ipad', 'This iPad', { kind: 'local', connected: true, path: null })
const mbp = machine('mbp', '333 · MacBook Pro', { hub: true, kind: 'hub' })
const p50 = machine('p50', 'P50 · ThinkPad')
const nyc = machine('nyc', 'cloud-nyc1', { kind: 'cloud' })
const xeus = machine('xeus', 'xeus', { online: false, path: null })

window.HTMLElement.prototype.scrollIntoView = () => {}
afterEach(() => {
  document.body.innerHTML = ''
})

/** The Defaults tab's title-model row on P50, with the page's sheet; `values` is what `settings_values` answers. */
async function mount(values: Record<string, Record<string, unknown>>, tab = 'helpers') {
  const settings_values = vi.fn(async () => values)
  const api = { settings_values, engine_call_all: vi.fn(async () => []) } as unknown as FullApi
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  function Page() {
    const [request, setRequest] = useState<SetOnAllRequest | null>(null)
    const settings: SettingsMachines = { machines: [ipad, mbp, p50, nyc, xeus], target: p50, connected: ipad, api, qc, tab: placeOf(tab)!.tab, open: setRequest }
    const field = setterField('title-model', 'Title model', 'openai/gpt-6', 'set_title_model', { read: fromGetter('title_model', (raw) => (raw as { id: string } | null)?.id ?? null) })
    return h(SettingsMachinesProvider, { value: settings }, h(Row, { label: 'Title model', copy: field }), h(SetOnAllSheet, { request, settings, onClose: () => setRequest(null) }))
  }
  const host = document.createElement('div')
  document.body.append(host)
  await act(async () => createRoot(host).render(h(QueryClientProvider, { client: qc }, h(Page))))
  return { host, settings_values }
}

test('227: other machines holding another value show "differs on N"; the compare lists each; Use this everywhere opens the sheet with that value', async () => {
  const { host, settings_values } = await mount({
    ipad: { title_model: { id: 'openai/gpt-6' } },
    mbp: { title_model: { id: 'anthropic/opus-5.5' } },
    p50: { title_model: { id: 'openai/gpt-6' } },
    nyc: { title_model: { id: 'anthropic/sonnet-5.5' } },
  })
  const chip = () => host.querySelector<HTMLButtonElement>('[data-differs]')
  await vi.waitFor(() => expect(chip()?.textContent).toBe('differs on 2'))
  // The reachable machines only (xeus is offline), one getter.
  expect(settings_values).toHaveBeenCalledWith(['ipad', 'mbp', 'p50', 'nyc'], ['title_model'])
  await act(async () => chip()!.click())
  const compare = (id: string) => document.querySelector<HTMLElement>(`[data-compare="${id}"]`)!
  await vi.waitFor(() => expect(compare('mbp')).not.toBeNull())
  expect(['ipad', 'mbp', 'p50', 'nyc'].map((id) => compare(id).querySelector('[data-machine] > span:last-child')?.textContent)).toEqual([
    'openai/gpt-6',
    'anthropic/opus-5.5',
    'openai/gpt-6',
    'anthropic/sonnet-5.5',
  ])
  await act(async () => [...compare('mbp').querySelectorAll('button')].find((b) => b.textContent === 'Use this everywhere')!.click())
  await vi.waitFor(() => expect(document.querySelector('[data-set-on-all]')).not.toBeNull())
  expect(document.querySelector('[data-set-on-all] [data-slot="sheet-description"]')?.textContent).toBe('Title model → anthropic/opus-5.5')
})

test('227: the same value everywhere shows no chip; a tab that is not copyable shows no copy controls at all', async () => {
  const same = { ipad: { title_model: { id: 'openai/gpt-6' } }, mbp: { title_model: { id: 'openai/gpt-6' } } }
  const { host, settings_values } = await mount(same)
  await vi.waitFor(() => expect(settings_values).toHaveBeenCalled())
  // Let the answer land and render.
  await act(async () => {})
  expect(host.querySelector('[data-copy-controls]')).not.toBeNull()
  expect(host.querySelector('[data-differs]')).toBeNull()
  document.body.innerHTML = ''
  const harnesses = await mount(same, 'harnesses')
  expect(harnesses.host.querySelector('[data-copy-controls]')).toBeNull()
})
