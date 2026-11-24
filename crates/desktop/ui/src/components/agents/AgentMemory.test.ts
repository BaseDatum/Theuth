import '@/components/settings/test-dom'
import { QueryClient, QueryClientProvider, notifyManager } from '@tanstack/react-query'
import { RouterProvider, createMemoryHistory, createRootRoute, createRouter } from '@tanstack/react-router'
import { act, createElement as h, type ReactElement } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { SidebarProvider } from '@/components/ui/sidebar'
import { TooltipProvider } from '@/components/ui/tooltip'
import { memoryFixture, MEMORY_EMPTY, REFUSAL } from '@/dev/agent-memory-fixture'
import type * as Rpc from '@/lib/rpc'
import { useConnection } from '@/store/connection'
import { AgentDetail } from './AgentDetail'
import { AgentMemory } from './AgentMemory'
import { AgentsView } from './AgentsView'
import { HireWizard } from './HireWizard'
import { DRAFT_KEY, emptyDraft } from './wizard'

vi.hoisted(() => vi.stubGlobal('localStorage', { getItem: () => null, setItem: () => undefined }))
let fixture = memoryFixture()
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => fixture.api }))
vi.mock('@/lib/notify', () => ({ notify: Object.assign(vi.fn(), { error: vi.fn() }) }))
notifyManager.setScheduler(queueMicrotask)
window.HTMLElement.prototype.scrollIntoView = () => {}
window.scrollTo = () => {}
let root: Root | undefined
let qc: QueryClient
const storage = new Map<string, string>()
beforeEach(() => {
  storage.clear()
  vi.stubGlobal('localStorage', { getItem: (key: string) => storage.get(key) ?? null, setItem: (key: string, value: string) => storage.set(key, value), removeItem: (key: string) => storage.delete(key) })
  fixture = memoryFixture()
  useConnection.setState({ discovery: { ...useConnection.getState().discovery, phase: 'legacy' } })
})
afterEach(async () => {
  await act(async () => root?.unmount())
  root = undefined
  qc?.clear()
  document.body.innerHTML = ''
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
})
async function mount(element: ReactElement) {
  qc = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: Infinity }, mutations: { retry: false } } })
  const host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  const router = createRouter({ routeTree: createRootRoute({ component: () => element }), history: createMemoryHistory() })
  await act(async () => root!.render(h(QueryClientProvider, { client: qc }, h(TooltipProvider, null, h(SidebarProvider, null, h(RouterProvider, { router }))))))
  return host
}
const button = (text: string) => [...document.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent === text)!
const toggle = () => document.querySelector<HTMLButtonElement>('#rt-isolated-memory')!
const click = (element: HTMLElement) => act(async () => element.click())
async function input(value: string) {
  const field = document.querySelector<HTMLInputElement>('input[aria-label="Search this agent\'s memory"]')!
  await act(async () => {
    Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')!.set!.call(field, value)
    field.dispatchEvent(new Event('input', { bubbles: true }))
  })
}

test('runtime toggle saves isolated_memory through agent_put, then disabling hides the badge and Memory tab', async () => {
  await mount(h(AgentDetail, { name: 'research', tab: 'runtime' }))
  await vi.waitFor(() => expect(toggle()?.getAttribute('aria-checked')).toBe('true'))
  expect(document.getElementById('rt-isolated-help')?.textContent).toBe("Keeps its own memory. What it learns never reaches your main memory, and it doesn't see yours.")
  expect(document.querySelector('[role="tab"][data-state="inactive"][value="memory"]') ?? button('Memory')).toBeTruthy()
  await click(toggle())
  await click(button('Save changes'))
  await vi.waitFor(() => expect(fixture.calls.filter((c) => c.method === 'agent_put').at(-1)?.params[0]).toMatchObject({ name: 'research', isolated_memory: false }))
  await vi.waitFor(() => expect(button('Memory')).toBeUndefined())
  expect(document.querySelector('article')?.textContent?.match(/Isolated memory/g)).toHaveLength(1)
})

test('a runtime refusal is verbatim beneath the toggle and reverts the isolation choice', async () => {
  delete fixture.files.yang!.isolated_memory
  await mount(h(AgentDetail, { name: 'yang', tab: 'runtime' }))
  await vi.waitFor(() => expect(toggle()?.getAttribute('aria-checked')).toBe('false'))
  await click(toggle())
  await click(button('Save changes'))
  await vi.waitFor(() => expect(document.getElementById('rt-isolated-error')?.textContent).toBe(REFUSAL))
  expect(toggle().getAttribute('aria-checked')).toBe('false')
  expect(button('Save changes').disabled).toBe(true)
  expect(fixture.calls.find((c) => c.method === 'agent_put')?.params[0]).toMatchObject({ name: 'yang', isolated_memory: true })
})

test('hire runtime and review preserve the isolation choice, a refused hire returns to runtime and reverts it', async () => {
  fixture = memoryFixture('refuse')
  localStorage.setItem(DRAFT_KEY, JSON.stringify({ step: 'runtime', draft: { ...emptyDraft(), name: 'new-research', title: 'Researcher', body: 'Charter.' } }))
  await mount(h(HireWizard))
  await vi.waitFor(() => expect(toggle()).toBeTruthy())
  await click(toggle())
  await click(button('7Review'))
  await vi.waitFor(() => expect(document.querySelector('dl')?.textContent).toContain('Isolated memoryOn'))
  await click(button('Hire Researcher'))
  await vi.waitFor(() => expect(fixture.calls.find((c) => c.method === 'agent_put')?.params[0]).toMatchObject({ name: 'new-research', isolated_memory: true }))
  await vi.waitFor(() => expect(document.getElementById('rt-isolated-error')?.textContent).toBe(REFUSAL))
  expect(toggle().getAttribute('aria-checked')).toBe('false')
  expect(JSON.parse(localStorage.getItem(DRAFT_KEY)!).draft.isolated_memory).toBe(false)
})

test('list and header badges appear only on isolated agents; a non-isolated direct Memory URL never reads a bundle', async () => {
  localStorage.setItem('theuth.agents-view', 'list')
  const host = await mount(h(AgentsView))
  await vi.waitFor(() => expect(host.querySelectorAll('tbody tr')).toHaveLength(4))
  const rows = [...host.querySelectorAll('tbody tr')]
  expect(rows.filter((row) => row.textContent?.includes('Isolated memory')).map((row) => row.textContent)).toEqual([expect.stringContaining('Researcher')])
  await act(async () => root!.unmount())
  qc.clear()
  await mount(h(AgentDetail, { name: 'yang', tab: 'memory' }))
  await vi.waitFor(() => expect(document.querySelector('h3')?.textContent).toBe('SWE desk'))
  expect(button('Memory')).toBeUndefined()
  expect(document.querySelector('article')?.textContent).not.toContain('Isolated memory')
  expect(fixture.calls.some((c) => c.method.startsWith('knowledge_'))).toBe(false)
})

test('Memory lists, searches even one character, previews and follows links only inside agent:<name>', async () => {
  const host = await mount(h(AgentDetail, { name: 'research', tab: 'memory' }))
  await vi.waitFor(() => expect(button(fixture.notes[0]!.title + fixture.notes[0]!.id)).toBeTruthy())
  expect(host.textContent).toContain('Isolated memory')
  expect(fixture.calls).toContainEqual({ method: 'knowledge_list', params: [null, null, null, 100000, 'agent:research'] })
  expect(host.textContent).toContain('3 notes · stored on this machine only · read-only here')
  await click(button(fixture.notes[0]!.title + fixture.notes[0]!.id))
  await vi.waitFor(() => expect(host.querySelector('[data-testid="knowledge-readonly"]')?.textContent).toContain('Three vendors remain'))
  expect(fixture.calls).toContainEqual({ method: 'knowledge_get', params: [fixture.notes[0]!.id, 'agent:research'] })
  expect(host.querySelector('[contenteditable="true"]')).toBeNull()
  expect(button('missing note').disabled).toBe(true)
  expect(host.querySelector('[aria-label="Memory preview"] a[href^="/knowledge"]')).toBeNull()
  await click(button('planning paper'))
  await vi.waitFor(() => expect(fixture.calls).toContainEqual({ method: 'knowledge_get', params: [fixture.notes[1]!.id, 'agent:research'] }))
  await input('p')
  await vi.waitFor(() => expect(fixture.calls).toContainEqual({ method: 'knowledge_search', params: ['p', 50, 'agent:research'] }))
  await input('no-such-note')
  await vi.waitFor(() => expect(host.textContent).toContain('No matches'))
  await input('')
  await vi.waitFor(() => expect(button(fixture.notes[0]!.title + fixture.notes[0]!.id)).toBeTruthy())
  expect(qc.getQueryData(['knowledge', 'list'])).toBeUndefined()
  expect(qc.getQueryData(['knowledge', 'doc', fixture.notes[0]!.id])).toBeUndefined()
})

test('Memory empty state and read failure are inline, without retries or a main fallback', async () => {
  fixture = memoryFixture('empty')
  const host = await mount(h(AgentMemory, { name: 'research' }))
  await vi.waitFor(() => expect(host.textContent).toContain(MEMORY_EMPTY))
  expect(host.textContent).toContain('0 notes · stored on this machine only · read-only here')
  const failed = vi.spyOn(fixture.api, 'knowledge_list').mockRejectedValue(new Error('isolated memory unavailable here: fixture bundle is on another machine'))
  await act(async () => { await qc.invalidateQueries({ queryKey: ['knowledge', 'agent-memory'] }) })
  await vi.waitFor(() => expect(host.querySelector('[role="alert"]')?.textContent).toBe('isolated memory unavailable here: fixture bundle is on another machine'))
  expect(failed).toHaveBeenCalledExactlyOnceWith(null, null, null, 100000, 'agent:research')
})

test('preview errors stay inline and never fall back to the main document', async () => {
  const host = await mount(h(AgentMemory, { name: 'research' }))
  await vi.waitFor(() => expect(button(fixture.notes[0]!.title + fixture.notes[0]!.id)).toBeTruthy())
  vi.spyOn(fixture.api, 'knowledge_get').mockRejectedValueOnce(new Error('bundle read failed'))
  await click(button(fixture.notes[0]!.title + fixture.notes[0]!.id))
  await vi.waitFor(() => expect(host.querySelector('[role="alert"]')?.textContent).toBe('bundle read failed'))
  expect(host.querySelector('[data-testid="knowledge-readonly"]')).toBeNull()
  expect(qc.getQueryData(['knowledge', 'doc', fixture.notes[0]!.id])).toBeUndefined()
})

test('two agents with the same note id have separate list and preview caches', async () => {
  const host = await mount(h('div', null, h(AgentMemory, { name: 'research' }), h(AgentMemory, { name: 'yang' })))
  await vi.waitFor(() => expect(host.querySelectorAll('li button')).toHaveLength(6))
  const sections = host.querySelectorAll('section')
  await click(sections[0]!.querySelector<HTMLButtonElement>('li button')!)
  await click(sections[1]!.querySelector<HTMLButtonElement>('li button')!)
  await vi.waitFor(() => expect(host.querySelectorAll('[data-testid="knowledge-readonly"]')).toHaveLength(2))
  for (const name of ['research', 'yang']) {
    expect(fixture.calls).toContainEqual({ method: 'knowledge_get', params: [fixture.notes[0]!.id, `agent:${name}`] })
    expect(qc.getQueryData(['knowledge', 'agent-memory', name, 'list', ''])).toEqual(fixture.notes)
    expect(qc.getQueryData(['knowledge', 'agent-memory', name, 'doc', fixture.notes[0]!.id])).toMatchObject({ meta: fixture.notes[0] })
  }
})
