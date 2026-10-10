import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { useDrafts } from '@/lib/drafts'
import { useContextDefaults } from '@/lib/notebook-context'
import { ensureCreateOptions, useEnsureNotebookSession, useOpenThread } from './NotebookContext'

// Hoisted with the mocks so the store's `load()` at module init sees the remembered thread of `nb3`.
vi.hoisted(() => {
  const s: Record<string, string> = { 'notebook-chat': JSON.stringify({ nb3: 'kept1' }) }
  globalThis.localStorage = { getItem: (k: string) => s[k] ?? null, setItem: (k: string, v: string) => void (s[k] = v) } as unknown as Storage
})

// `vi.mock` is hoisted above the imports: the doubles the factories close over must be hoisted with them.
const m = vi.hoisted(() => ({
  api: {
    create_session: vi.fn(async (_opts: Record<string, unknown>) => ({ session: 'fresh1', retention: 'durable' })),
    rename_session: vi.fn(async () => null),
    endless_settings_get: vi.fn(async () => ({ default_chat: false })),
  },
  ctx: { notebook_context_set: vi.fn(async () => null) },
  invalidateQueries: vi.fn(),
  chats: [] as { id: string }[],
  /** What `useQuery` answers for `endless_settings_get` (the engine's `agent.endless.default_chat`). */
  endlessDefault: false,
}))
const { api, ctx, invalidateQueries } = m

vi.mock('@tanstack/react-query', () => ({
  useQueryClient: () => ({ invalidateQueries: m.invalidateQueries }),
  useQuery: () => ({ data: { default_chat: m.endlessDefault } }),
}))
vi.mock('@/lib/rpc', () => ({ useApi: () => m.api, queryKeys: { endlessSettings: ['endless-settings'] } }))
vi.mock('@/lib/notebooks-rpc', () => ({ useContextApi: () => m.ctx, notebookKeys: { chats: (id: string) => ['notebooks', 'chats', id] } }))
vi.mock('@/components/notebooks/useNotebook', () => ({
  useNotebook: () => ({ data: { doc: { title: 'Vendor review' } } }),
  useNotebookChats: () => ({ data: m.chats }),
}))

/** Renders the hook once and hands back what it returned (no DOM in this tree). */
function ensureFor(notebook: string): () => Promise<string> {
  let got: (() => Promise<string>) | undefined
  const Probe = () => {
    got = useEnsureNotebookSession(notebook)
    return null
  }
  renderToStaticMarkup(createElement(Probe))
  return got!
}

test('144 §5.1 (Z2): a dial touched before the first message creates the bound thread, titles it, writes the device defaults and remembers it', async () => {
  useContextDefaults.getState().set('nb1', 's1', 'source', 'off')
  useContextDefaults.getState().set('nb1', 'n1', 'note', 'full')
  const id = await ensureFor('nb1')()
  expect(id).toBe('fresh1')
  expect(api.create_session).toHaveBeenCalledTimes(1)
  expect(api.create_session.mock.calls[0]![0]).toMatchObject({ notebook: 'nb1', retention: 'durable', model: null, effort: null })
  expect(api.create_session.mock.calls[0]![0]).not.toHaveProperty('memory')
  expect(api.rename_session).toHaveBeenCalledWith('fresh1', 'Vendor review')
  expect(ctx.notebook_context_set.mock.calls.map((c) => c.slice(0, 4))).toEqual([
    ['fresh1', 's1', 'source', 'off'],
    ['fresh1', 'n1', 'note', 'full'],
  ])
  expect(useOpenThread.getState().open['nb1']).toBe('fresh1')
  expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ['notebooks', 'chats', 'nb1'] })
})

test('an existing thread — remembered on this device or the newest chat — is answered; nothing is created', async () => {
  // `renderToStaticMarkup` reads a zustand store's initial state, hence the seed before the module loaded.
  expect(await ensureFor('nb3')()).toBe('kept1')
  m.chats.push({ id: 'chat9' })
  expect(await ensureFor('nb2')()).toBe('chat9')
  expect(api.create_session).not.toHaveBeenCalled()
  m.chats.length = 0
})

test('157 D2: the dial path sends the draft pick and the ∞ choice, then spends the pick like the composer path', async () => {
  useDrafts.getState().setPick({ model: 'openai/gpt-5.4', effort: 'high' })
  useOpenThread.getState().setEndless('nb4', true)
  await ensureFor('nb4')()
  expect(api.create_session.mock.calls[0]![0]).toMatchObject({ notebook: 'nb4', model: 'openai/gpt-5.4', effort: 'high', memory: 'endless' })
  expect(useDrafts.getState().pick).toEqual({ model: null, effort: null })
  // Remembering the created thread ends the draft: its ∞ choice is forgotten.
  expect(useOpenThread.getState().endless['nb4']).toBeUndefined()
})

test('157 D3: the ∞ default is the engine setting until the draft is touched; New chat (set null) forgets the touch', async () => {
  m.endlessDefault = true
  await ensureFor('nb5')()
  expect(api.create_session.mock.calls[0]![0]).toMatchObject({ memory: 'endless' })
  useOpenThread.getState().set('nb5', null)
  useOpenThread.getState().setEndless('nb5', false)
  await ensureFor('nb5')()
  expect(api.create_session.mock.calls[1]![0]).not.toHaveProperty('memory')
  useOpenThread.getState().set('nb5', null)
  expect(useOpenThread.getState().endless['nb5']).toBeUndefined()
  m.endlessDefault = false
})

test('ensureCreateOptions is pure: pick and notebook always, memory only when endless is on', () => {
  expect(ensureCreateOptions('nb', { model: null, effort: null }, false)).toEqual({
    retention: 'durable',
    project: null,
    notebook: 'nb',
    view: null,
    cwd: null,
    model: null,
    effort: null,
    parent_session: null,
    parent_engine: null,
    harness: null,
  })
  expect(ensureCreateOptions('nb', { model: 'm', effort: 'low' }, true)).toMatchObject({ notebook: 'nb', model: 'm', effort: 'low', memory: 'endless' })
})
