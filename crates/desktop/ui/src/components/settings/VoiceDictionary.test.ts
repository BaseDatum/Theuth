import { readyConnection } from '@/components/settings/test-engine'
import type * as ConnectionModule from '@/store/connection'
import './test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createContext, createElement, type ReactNode, useContext } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type { Entry } from "@/rpc/bindings/method.talk.output"
import { BUILTIN_LIMIT, VoiceDictionary, entryFromForm, filterEntries, formFor, heardForms, spokenForms } from './VoiceDictionary'

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn() }) }))
// The select registers its setter under its trigger's label (Radix does not open in jsdom).
const selects: Record<string, (v: string) => void> = {}
const SelectCtx = createContext<{ value?: string; onValueChange?: (v: string) => void }>({})
vi.mock('@/components/ui/select', () => ({
  Select: (p: { value?: string; onValueChange?: (v: string) => void; children: ReactNode }) => createElement(SelectCtx.Provider, { value: p }, p.children),
  SelectTrigger: (p: { 'aria-label'?: string }) => {
    const c = useContext(SelectCtx)
    if (p['aria-label'] && c.onValueChange) selects[p['aria-label']] = c.onValueChange
    return createElement('span', { 'aria-label': p['aria-label'], 'data-value': c.value })
  },
  SelectValue: () => null,
  SelectContent: () => null,
  SelectItem: () => null,
}))

const entry = (over: Partial<Entry>): Entry => ({ id: 'x', canonical: 'X', kind: 'term', spoken: [], variants: [], scope: { scope: 'global' }, source: 'user', tags: [], ...over })
const variant = (heard: string) => ({ heard, metaphone: ['', ''] as [string, string], seen: 1, last_ms: 0 })
const THEUTH = entry({ id: 'u1', canonical: 'Theuth', spoken: ['tooth', 'truth'], variants: [variant('toot'), variant('Tooth'), variant('tuth'), variant('thoth')] })
const KUBECTL = entry({ id: 'l1', canonical: 'kubectl', source: 'learned', spoken: ['cube control'], scope: { scope: 'surface', surface: 'tui' }, tags: ['keep_case'] })
const builtIn = (i: number) => entry({ id: `p${i}`, canonical: `Git${i}`, source: 'packaged', spoken: ['git hub'] })

let stored: Entry[] = []
const api = {
  dictionary_list: vi.fn(async () => stored),
  dictionary_counts: vi.fn(async () => ({ user: 1, learned: 1, packaged: 40_641 })),
  dictionary_search: vi.fn(async (_q: string, _s: string, limit: number) => ({ entries: Array.from({ length: limit }, (_, i) => builtIn(i)), total: 240 })),
  dictionary_put: vi.fn(async (e: Entry) => ({ ...e, id: e.id || 'minted' })),
  dictionary_delete: vi.fn(async () => true),
}
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => api }))
// The engine's capabilities come from the exported contract, so `useQuerySupport` sees the dictionary methods.
vi.mock('@/store/connection', async (original) => {
  const m = await original<typeof ConnectionModule>()
  return { ...m, useConnection: readyConnection(m.createConnection()) }
})

let root: Root
let host: HTMLElement
beforeEach(async () => {
  stored = [THEUTH, KUBECTL]
  vi.clearAllMocks()
  host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  await act(async () => root.render(createElement(QueryClientProvider, { client: qc, children: createElement(VoiceDictionary) as ReactNode })))
  await settle()
})
afterEach(() => {
  act(() => root.unmount())
  host.remove()
})

/** React Query notifies observers on a 0 ms timer (its default scheduler), so a microtask flush is not enough: yield a few macrotasks. */
async function settle() {
  for (let i = 0; i < 5; i++)
    await act(async () => {
      const { promise, resolve } = Promise.withResolvers<void>()
      setTimeout(resolve, 0)
      await promise
    })
}
const rows = () => [...host.querySelectorAll('[data-dictionary-row]')].map((r) => r.textContent ?? '')
const button = (label: string) => [...host.querySelectorAll('button')].find((b) => b.textContent?.startsWith(label) || b.getAttribute('aria-label') === label) as HTMLButtonElement
async function click(el: Element) {
  await act(async () => (el as HTMLElement).click())
  await settle()
}
async function type(label: string, value: string) {
  const input = host.querySelector(`input[aria-label="${label}"]`) as HTMLInputElement
  Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set?.call(input, value)
  await act(async () => input.dispatchEvent(new window.Event('input', { bubbles: true })))
  await settle()
}

test('rows: canonical, up to 4 heard chips then +N, scope and source tags; Yours + Learned on, Built-in off with its count', () => {
  const [theuth, kubectl] = rows()
  expect(theuth).toContain('Theuth')
  // tooth, truth, toot, tuth (Tooth folds into tooth) then +1 (thoth).
  expect(theuth).toMatch(/toothtruthtoottuth\+1EverywhereYours/)
  expect(kubectl).toMatch(/kubectlcube controlTerminalLearned/)
  expect(button('Yours').getAttribute('aria-pressed')).toBe('true')
  expect(button('Learned').getAttribute('aria-pressed')).toBe('true')
  expect(button('Built-in').getAttribute('aria-pressed')).toBe('false')
  expect(button('Built-in').textContent).toContain('40641')
  expect(api.dictionary_search).not.toHaveBeenCalled()
  expect(host.textContent).toContain('Delete entry')
})

test('search filters canonical / spoken / heard case-insensitively; a source chip hides its rows', async () => {
  await type('Search dictionary', 'TUTH')
  expect(rows()).toHaveLength(1)
  await type('Search dictionary', 'cube')
  expect(rows()[0]).toContain('kubectl')
  await type('Search dictionary', '')
  await click(button('Learned'))
  expect(rows()).toHaveLength(1)
  expect(rows()[0]).toContain('Theuth')
})

test('built-in: queried only with its chip on and 2+ characters; capped at 100 with the refine line; Override pre-fills a User entry', async () => {
  await click(button('Built-in'))
  await type('Search dictionary', 'g')
  expect(api.dictionary_search).not.toHaveBeenCalled()
  expect(host.textContent).toContain('Type at least 2 characters')
  await type('Search dictionary', 'gi')
  expect(api.dictionary_search).toHaveBeenLastCalledWith('gi', 'packaged', BUILTIN_LIMIT)
  const builtInRows = rows().filter((r) => r.includes('Built-in'))
  expect(builtInRows).toHaveLength(100)
  expect(builtInRows[0]).toContain('Override')
  expect(builtInRows[0]).not.toContain('Delete entry')
  expect(builtInRows[0]).not.toContain('Edit')
  expect(host.textContent).toContain('Showing 100 of 240 — refine the search')
  await click(button('Override'))
  expect((host.querySelector('input[aria-label="Written as"]') as HTMLInputElement).value).toBe('Git0')
  await click(button('Save'))
  expect(api.dictionary_put).toHaveBeenLastCalledWith(expect.objectContaining({ id: '', canonical: 'Git0', source: 'user', spoken: ['git hub'] }))
})

test('add: an empty "Written as" cannot save; a new entry goes out with an empty id, User source, scope and keep_case', async () => {
  await click(button('Add entry'))
  expect(button('Save').disabled).toBe(true)
  await type('Written as', 'GitHub')
  await type('Heard as', 'Git Hub, get hub, , git hub')
  await act(async () => selects.Applies?.('composer'))
  await click(host.querySelector('input[aria-label="Keep exact casing"]') as HTMLInputElement)
  await click(button('Save'))
  expect(api.dictionary_put).toHaveBeenLastCalledWith({
    id: '',
    canonical: 'GitHub',
    kind: 'term',
    spoken: ['git hub', 'get hub'],
    variants: [],
    scope: { scope: 'surface', surface: 'composer' },
    source: 'user',
    tags: ['keep_case'],
  })
  expect(host.querySelector('form')).toBeNull()
})

test('edit keeps the id; renaming deletes the old row first; Delete entry deletes', async () => {
  await click(host.querySelectorAll('[data-dictionary-row]')[0]?.querySelectorAll('button')[0] as Element)
  await type('Written as', 'Theuth ')
  await click(button('Save'))
  expect(api.dictionary_delete).not.toHaveBeenCalled()
  expect(api.dictionary_put).toHaveBeenLastCalledWith(expect.objectContaining({ id: 'u1', canonical: 'Theuth', variants: THEUTH.variants }))
  await click(host.querySelectorAll('[data-dictionary-row]')[0]?.querySelectorAll('button')[0] as Element)
  await type('Written as', 'Thoth')
  await click(button('Save'))
  expect(api.dictionary_delete).toHaveBeenCalledWith('u1')
  expect(api.dictionary_put).toHaveBeenLastCalledWith(expect.objectContaining({ id: 'u1', canonical: 'Thoth' }))
  await click(host.querySelectorAll('[data-dictionary-row]')[1]?.querySelectorAll('button')[1] as Element)
  expect(api.dictionary_delete).toHaveBeenLastCalledWith('l1')
})

test('empty state', async () => {
  stored = []
  act(() => root.unmount())
  root = createRoot(host)
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  await act(async () => root.render(createElement(QueryClientProvider, { client: qc, children: createElement(VoiceDictionary) as ReactNode })))
  await settle()
  expect(host.textContent).toContain('No entries yet. Add the names and terms dictation gets wrong, or correct a dictation and choose Add when asked.')
})

test('helpers: heard forms de-duplicate, spoken forms normalise, Edit/Override forms map back', () => {
  expect(heardForms(THEUTH)).toEqual(['tooth', 'truth', 'toot', 'tuth', 'thoth'])
  expect(spokenForms(' A, b ,, a ')).toEqual(['a', 'b'])
  expect(filterEntries([THEUTH, KUBECTL], 'THOTH', new Set(['user']))).toEqual([THEUTH])
  expect(entryFromForm(formFor(KUBECTL, false))).toMatchObject({ id: 'l1', source: 'user', scope: { scope: 'surface', surface: 'tui' }, tags: ['keep_case'] })
  expect(entryFromForm(formFor(builtIn(0), true))).toMatchObject({ id: '', source: 'user', scope: { scope: 'global' }, variants: [] })
})
