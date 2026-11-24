import '../settings/test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { TooltipProvider } from '@/components/ui/tooltip'
import type * as Rpc from '@/lib/rpc'
import { NO_PICK, useDrafts } from '@/lib/drafts'
import { EMPTY, useNoteChat } from '@/lib/note-chat'
import type { ModelInfo } from '@/rpc/bindings/method.agent.output'
import { useConnection } from '@/store/connection'
import { useSessions } from '@/store/sessions'
import { NoteChat } from './NoteChat'

const rpc = vi.hoisted(() => ({ api: {} as Record<string, unknown> }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => rpc.api }))
vi.mock('@tanstack/react-router', () => ({ useNavigate: () => () => undefined, useRouterState: () => '/' }))
// The panel's composer and transcript have their own tests; this one is about the model pick. 215 C3: the picker is the
// composer's (`ComposerModel` on its toolbar), so the stub composer renders just that label (imported inside the hoisted
// factory: `vi.mock` runs before the file's static imports exist).
vi.mock('@/components/chat/Composer', async () => {
  const { ComposerModel } = await import('@/components/chat/ModelMenu')
  const { createElement } = await import('react')
  return { Composer: ({ session }: { session: string | null }) => createElement('div', { 'data-composer': '' }, createElement(ComposerModel, { session })) }
})
vi.mock('@/components/chat/Transcript', () => ({ Transcript: () => null }))
vi.mock('@/components/chat/ToolsSheet', () => ({ ToolsSheet: () => null }))
vi.mock('@/components/chat/advisor', () => ({ useAdvisor: () => undefined, useSetAdvisor: () => ({ mutate: () => {} }) }))
// A connected engine that advertises the picker's methods (the gate itself has its own tests).
vi.mock('@/rpc/capabilities', async (original) => ({ ...(await original<object>()), Capability: ({ children }: { children: unknown }) => children }))

window.HTMLElement.prototype.scrollIntoView = () => {}

const catalog: ModelInfo[] = [
  { id: 'openai/gpt-5.4', name: 'GPT-5.4', provider: 'openai', context: 0, reasoning: false, image: false, efforts: [], budget_min: null, budget_max: null, local: false },
  { id: 'anthropic/claude-opus-4-1', name: 'Claude Opus 4.1', provider: 'anthropic', context: 0, reasoning: false, image: false, efforts: [], budget_min: null, budget_max: null, local: false },
]
const doc = { kind: 'note' as const, id: 'n1', title: 'Plan', meta: '' }
const calls: unknown[][] = []

beforeEach(() => {
  calls.length = 0
  useConnection.setState({ discovery: { ...useConnection.getState().discovery, phase: 'legacy' }, info: { favorite_models: [], default_model: 'anthropic/claude-opus-4-1' } as never })
  useDrafts.setState({ pick: NO_PICK, settings: {} })
  rpc.api = {
    list_sessions: async () => [],
    list_models: async () => catalog,
    load_session: () => new Promise(() => {}),
    set_model: async (s: string, id: string) => void calls.push(['set_model', s, id]),
  }
})
afterEach(() => {
  document.body.innerHTML = ''
})

async function mount() {
  const host = document.createElement('div')
  document.body.append(host)
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  await act(async () => createRoot(host).render(createElement(QueryClientProvider, { client: qc }, createElement(TooltipProvider, null, createElement(NoteChat, { note: 'n1', doc })))))
  const trigger = () => host.querySelector<HTMLButtonElement>('[data-composer] button')!
  const click = (el: Element) => act(async () => void (el as HTMLElement).click())
  const pickModel = async (provider: string, id: string) => {
    await click(trigger())
    await click(document.querySelector(`[data-value="provider:${provider}"]`)!)
    await click(document.querySelector(`[data-value="model:${id}"]`)!)
  }
  // 215 C3: the header carries no picker of its own.
  const headerText = () => host.querySelector('header')!.textContent
  return { trigger, pickModel, headerText }
}

test('#8: before its first message the note chat picks a model for the chat it will create (the draft pick), as the main composer does', async () => {
  useNoteChat.setState({ notes: { n1: { ...EMPTY, session: null } } })
  const p = await mount()
  await vi.waitFor(() => expect(p.trigger().textContent).toBe('Claude Opus 4.1'))
  expect(p.headerText()).not.toContain('Claude')
  await p.pickModel('openai', 'openai/gpt-5.4')
  expect(useDrafts.getState().pick).toEqual({ model: 'openai/gpt-5.4', effort: null })
  expect(calls).toEqual([])
  await vi.waitFor(() => expect(p.trigger().textContent).toBe('GPT-5.4'))
})

test('#8: with a session the note chat’s picker shows and switches that session’s model', async () => {
  useSessions.setState((s) => ({ sessions: { ...s.sessions, s1: { ...s.sessions.s1, model: 'anthropic/claude-opus-4-1', effort: null } as never } }))
  useNoteChat.setState({ notes: { n1: { ...EMPTY, session: 's1' } } })
  const p = await mount()
  await vi.waitFor(() => expect(p.trigger().textContent).toBe('Claude Opus 4.1'))
  expect(p.headerText()).not.toContain('Claude')
  await p.pickModel('openai', 'openai/gpt-5.4')
  expect(calls).toEqual([['set_model', 's1', 'openai/gpt-5.4']])
})
