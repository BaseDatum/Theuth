import '../settings/test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type { FirstWaiting } from '@/rpc/bindings/FirstWaiting'
import { toast } from 'sonner'
import { useToolRuns } from '@/lib/tool-runs'
import { ContactAllowStrip } from './ContactAllowStrip'

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn(), message: vi.fn(), success: vi.fn() }) }))
const W: FirstWaiting = { contact: 'sky', machine: "Sky's Mac", export: 'ops', session: 's1', since_ms: 1, waiting: 1, preview: 'ping from sky' }
const api = {
  contact_first_waiting: vi.fn(async (): Promise<FirstWaiting[]> => [W]),
  contact_first_answer: vi.fn(async (_c: string, _e: string, _a: string) => null),
}
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => api }))

const tick = () => new Promise<void>((r) => setTimeout(r, 0))
const run = (fn: () => unknown) =>
  act(async () => {
    await fn()
    await tick()
  })
const body = () => document.body.textContent ?? ''
const click = (label: string) => run(() => [...document.querySelectorAll('button')].find((b) => b.textContent === label || b.getAttribute('aria-label') === label)!.click())
let unmount = async () => {}
let client: QueryClient
async function mount(session = 's1') {
  const host = document.createElement('div')
  document.body.append(host)
  const root = createRoot(host)
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  await run(() => root.render(createElement(QueryClientProvider, { client, children: createElement(ContactAllowStrip, { session }) })))
  await run(() => undefined)
  unmount = () => run(() => root.unmount())
}
beforeEach(() => {
  vi.clearAllMocks()
  useToolRuns.setState({ disclosed: {} })
  api.contact_first_waiting.mockImplementation(async () => [W])
})
afterEach(async () => {
  await unmount()
  document.body.innerHTML = ''
})

test('one waiting contact: a collapsed row with three answers and no share mode; expanding adds the preview', async () => {
  await mount()
  expect(body()).toContain("sky (Sky's Mac) wants to message this chat")
  expect(body()).not.toContain('ping from sky')
  await click('Details')
  expect(body()).toContain('ping from sky')
  for (const gone of ['Cautious', 'Full collaboration', 'limited turn']) expect(body()).not.toContain(gone)
})

test.each([
  ['Allow once', 'ask', 'Allowed sky once'],
  ['Always', 'allow', 'Allowed sky · always'],
  ['Block', 'block', 'Blocked sky'],
])('%s answers %s and toasts', async (label, answer, text) => {
  await mount()
  await click(label)
  expect(api.contact_first_answer).toHaveBeenCalledWith('sky', 'ops', answer)
  expect(toast).toHaveBeenCalledWith(text)
  // Answered: until the list drops it, a second click cannot set the standing default by accident.
  const answers = [...document.querySelectorAll('button')].filter((b) => ['Allow once', 'Always', 'Block'].includes(b.textContent ?? ''))
  expect(answers.map((b) => b.disabled)).toEqual([true, true, true])
})

test('several first messages from one contact: the row says how many', async () => {
  api.contact_first_waiting.mockImplementation(async () => [{ ...W, waiting: 3 }])
  await mount()
  expect(body()).toContain("sky (Sky's Mac) wants to message this chat · 3 messages")
})

test('several waiting: collapsed "N contacts waiting"; expanded one row each with its own buttons', async () => {
  api.contact_first_waiting.mockImplementation(async () => [W, { ...W, contact: 'eve', machine: '', export: 'docs' }])
  await mount()
  expect(body()).toContain('2 contacts waiting')
  expect(document.querySelectorAll('button')).toHaveLength(1)
  await click('Details')
  expect(body()).toContain('eve wants to message this chat')
  await run(() => [...document.querySelectorAll('button')].filter((b) => b.textContent === 'Block')[1]!.click())
  expect(api.contact_first_answer).toHaveBeenCalledWith('eve', 'docs', 'block')
})

test('nothing waits for this session: no strip', async () => {
  await mount('other')
  expect(document.querySelector('[role=region]')).toBeNull()
})

const Q: FirstWaiting = { ...W, contact: 'kim', preview: 'is the build green?', queued: true, position: 1 }

test('190: a queued row (a 186 engine in the mesh still sends them) never shows: alone it draws no strip', async () => {
  api.contact_first_waiting.mockImplementation(async () => [Q, { ...Q, export: 'docs', waiting: 2, position: 2 }])
  await mount()
  expect(document.querySelector('[role=region]')).toBeNull()
})

test('190: beside a held row a queued one adds nothing: one row, its own three answers', async () => {
  api.contact_first_waiting.mockImplementation(async () => [Q, W])
  await mount()
  expect(body()).not.toContain('contacts waiting')
  expect(body()).toContain("sky (Sky's Mac) wants to message this chat")
  await click('Details')
  for (const gone of ['kim', 'is the build green?', 'wait for this turn', 'waits for this turn']) expect(body()).not.toContain(gone)
  expect([...document.querySelectorAll('button')].filter((b) => b.textContent === 'Allow once')).toHaveLength(1)
})
