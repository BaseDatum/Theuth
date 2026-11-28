// New-chat model picker stability during first send: choosing a non-default model/effort on `<Composer session={null} />`
// and submitting must never flash back to the engine's default while `send_input` or `navigate` is in flight.
import '../settings/test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement as h, type ReactNode } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, expect, type Mock, test, vi } from 'vitest'
import { TooltipProvider } from '@/components/ui/tooltip'
import { EMPTY, NO_PICK, useDraftTarget, useDrafts } from '@/lib/drafts'
import type * as Rpc from '@/lib/rpc'
import type { ModelInfo } from '@/rpc/bindings/method.agent.output'
import { useConnection } from '@/store/connection'
import { useSessions } from '@/store/sessions'
import { Composer } from './Composer'
import { modelRequirements } from './requirements'

const catalog: ModelInfo[] = [
  { id: 'anthropic/claude-opus-4-1', name: 'Claude Opus 4.1', provider: 'anthropic', context: 0, reasoning: false, image: false, efforts: [], budget_min: null, budget_max: null, local: false },
  { id: 'openai/gpt-5.4', name: 'GPT-5.4', provider: 'openai', context: 0, reasoning: true, image: false, efforts: ['low', 'high'], budget_min: null, budget_max: null, local: false },
]

function deferred<T = void>() {
  let resolve!: (v: T) => void
  let reject!: (e: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

const h0 = vi.hoisted(() => {
  const calls = {
    list_models: vi.fn(),
    create_session: vi.fn(),
    new_session: vi.fn(),
    send_input: vi.fn(),
  }
  const rest = new Map<string, Mock>()
  const api = new Proxy(calls as Record<string, Mock>, {
    get: (t, k: string) => {
      if (k in t) return t[k]
      if (!rest.has(k)) rest.set(k, vi.fn(async () => (k.startsWith('list_') ? [] : null)))
      return rest.get(k)
    },
  })
  const navigate = vi.fn()
  return { calls, api, navigate }
})

const api = h0.calls
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => h0.api }))
vi.mock('@tanstack/react-router', async (original) => ({ ...(await original<object>()), useNavigate: () => h0.navigate, useRouterState: () => '/new' }))
vi.mock('@/lib/notify', () => ({ notify: Object.assign(vi.fn(), { success: vi.fn(), warning: vi.fn(), error: vi.fn(), message: vi.fn() }) }))
vi.mock('@/lib/speech', async (original) => ({ ...(await original<object>()), useVoice: () => false, release: async () => undefined }))

window.HTMLElement.prototype.scrollIntoView = () => {}

let root: Root | null = null
let host: HTMLDivElement
let qc: QueryClient

function setupEngine() {
  const methods = [...modelRequirements.methods, 'list_sessions', 'list_skills', 'vision_model', 'session_knowledge_harvester', 'set_session_knowledge_harvester', 'knowledge_harvester_model', 'subscribe_topics', 'unsubscribe_topics']
  const document = {
    methods: methods.map((name) => ({ name, params: [], result: {} })),
    'x-theuth': { subscriptions: [{ subscribe: 'subscribe_topics', unsubscribe: 'unsubscribe_topics', notification: 'subscribe_topics' }], events: [{ topic: 'agent.events', version: 1 }] },
  } as never
  useConnection.setState({
    discovery: { phase: 'ready', capabilities: null, document, unavailable: new Set() },
    info: { default_model: 'anthropic/claude-opus-4-1', default_effort: null, favorite_models: [] } as never,
  })
}

async function renderComposer(session: string | null) {
  await act(async () => {
    root!.render(h(QueryClientProvider, { client: qc }, h(TooltipProvider, null, h(Composer, { session, pane: null }) as ReactNode)))
  })
}

async function mountNew() {
  host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  qc = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } })
  await renderComposer(null)
  await vi.waitFor(() => expect(modelTrigger().textContent).toBe('Claude Opus 4.1'))
}

const modelTrigger = () => host.querySelector<HTMLButtonElement>('[data-tour="chat-model"]')!
const sendButton = () => host.querySelector<HTMLButtonElement>('button[aria-label="Send"]')!
const click = async (el: Element) => {
  await act(async () => void (el as HTMLElement).click())
}

async function selectGptHighAndType(text: string) {
  await click(modelTrigger())
  await click(document.querySelector('[data-value="provider:openai"]')!)
  await click(document.querySelector('[data-value="model:openai/gpt-5.4"]')!)
  await vi.waitFor(() => expect(document.querySelector('[aria-label="Effort"]')).not.toBeNull())
  await click(document.querySelector('[aria-label="Effort"] [data-value="effort:high"]')!)
  expect(modelTrigger().textContent).toBe('GPT-5.4· high')
  await act(async () => useDrafts.getState().setText('', text))
}

beforeEach(() => {
  localStorage.clear()
  useDrafts.setState({ by: { '': EMPTY }, edits: {}, pick: NO_PICK, settings: {}, opening: null })
  useDraftTarget.getState().set({ engine: null, cwd: null, reuse: null })
  useSessions.setState({ sessions: {} })
  for (const f of Object.values(api)) f.mockReset().mockResolvedValue(null)
  api.list_models.mockResolvedValue(catalog)
  h0.navigate.mockReset().mockResolvedValue(undefined)
  setupEngine()
})

afterEach(() => {
  act(() => root?.unmount())
  root = null
  host.remove()
  document.body.innerHTML = ''
})

test('new chat send: model picker stays on chosen model/effort while create_session, send_input, and navigate are in flight, then spends draft and hands off cleanly', async () => {
  await mountNew()
  await selectGptHighAndType('hello world')

  const createDef = deferred<{ session: string; retention: 'durable'; root: string; lease_secret: null }>()
  const sendDef = deferred<string>()
  const navDef = deferred<void>()
  api.create_session.mockReturnValue(createDef.promise)
  api.send_input.mockReturnValue(sendDef.promise)
  h0.navigate.mockReturnValue(navDef.promise)

  // Observe every DOM mutation on the model picker trigger throughout the send lifecycle: it must never flash to Claude Opus 4.1.
  const seenLabels: string[] = [modelTrigger().textContent ?? '']
  const observer = new MutationObserver(() => {
    seenLabels.push(modelTrigger().textContent ?? '')
  })
  observer.observe(modelTrigger(), { subtree: true, characterData: true, childList: true })

  await click(sendButton())
  expect(api.create_session).toHaveBeenCalledTimes(1)
  expect(modelTrigger().textContent).toBe('GPT-5.4· high')

  // Resolve create_session: send_input is now in flight while Composer(session=null) is still mounted.
  await act(async () => {
    createDef.resolve({ session: 's-created', retention: 'durable', root: 's-created', lease_secret: null })
    await Promise.resolve()
  })
  await vi.waitFor(() => expect(api.send_input).toHaveBeenCalledTimes(1))
  expect(modelTrigger().textContent).toBe('GPT-5.4· high')
  expect(useDrafts.getState().pick).toEqual({ model: 'openai/gpt-5.4', effort: 'high' })

  // Resolve send_input: navigate is now in flight; draft still not spent until navigate completes.
  await act(async () => {
    sendDef.resolve('ok')
    await Promise.resolve()
  })
  await vi.waitFor(() => expect(h0.navigate).toHaveBeenCalledWith({ to: '/c/$session', params: { session: 's-created' } }))
  expect(modelTrigger().textContent).toBe('GPT-5.4· high')
  expect(useDrafts.getState().pick).toEqual({ model: 'openai/gpt-5.4', effort: 'high' })

  observer.disconnect()
  expect(seenLabels.every((l) => l === 'GPT-5.4· high')).toBe(true)

  // Resolve navigate: draft is spent, and the mounted chat route composer renders the chosen model/effort.
  await act(async () => {
    navDef.resolve()
    await Promise.resolve()
  })
  await vi.waitFor(() => expect(useDrafts.getState().pick).toEqual(NO_PICK))

  await renderComposer('s-created')
  expect(modelTrigger().textContent).toBe('GPT-5.4· high')
})

test('new chat send_input rejection after create_session: no flash while pending, navigates to created session, and spends draft afterwards', async () => {
  await mountNew()
  await selectGptHighAndType('retry me')

  const sendDef = deferred<string>()
  const navDef = deferred<void>()
  api.create_session.mockResolvedValue({ session: 's-fail-send', retention: 'durable', root: 's-fail-send', lease_secret: null })
  api.send_input.mockReturnValue(sendDef.promise)
  h0.navigate.mockReturnValue(navDef.promise)

  await click(sendButton())
  await vi.waitFor(() => expect(api.send_input).toHaveBeenCalledTimes(1))
  expect(modelTrigger().textContent).toBe('GPT-5.4· high')
  expect(useDrafts.getState().pick).toEqual({ model: 'openai/gpt-5.4', effort: 'high' })

  // Reject send_input: composer navigates to the already-created session and keeps pick intact until navigate finishes.
  await act(async () => {
    sendDef.reject(new Error('boom'))
    await Promise.resolve()
  })
  await vi.waitFor(() => expect(h0.navigate).toHaveBeenCalledWith({ to: '/c/$session', params: { session: 's-fail-send' } }))
  expect(modelTrigger().textContent).toBe('GPT-5.4· high')

  await act(async () => {
    navDef.resolve()
    await Promise.resolve()
  })
  await vi.waitFor(() => expect(useDrafts.getState().pick).toEqual(NO_PICK))

  await renderComposer('s-fail-send')
  expect(modelTrigger().textContent).toBe('GPT-5.4· high')
})

test('new chat create_session rejection: chosen model/effort stays selected and pick remains intact for retry', async () => {
  await mountNew()
  await selectGptHighAndType('try again')

  api.create_session.mockRejectedValueOnce(new Error('offline'))
  await click(sendButton())
  await vi.waitFor(() => expect(api.create_session).toHaveBeenCalledTimes(1))

  expect(h0.navigate).not.toHaveBeenCalled()
  expect(modelTrigger().textContent).toBe('GPT-5.4· high')
  expect(useDrafts.getState().pick).toEqual({ model: 'openai/gpt-5.4', effort: 'high' })
})
