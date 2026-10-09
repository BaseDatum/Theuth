import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { useContextDefaults } from '@/lib/notebook-context'
import { useEnsureNotebookSession, useOpenThread } from './NotebookContext'

// Hoisted with the mocks so the store's `load()` at module init sees the remembered thread of `nb3`.
vi.hoisted(() => {
  const s: Record<string, string> = { 'notebook-chat': JSON.stringify({ nb3: 'kept1' }) }
  globalThis.localStorage = { getItem: (k: string) => s[k] ?? null, setItem: (k: string, v: string) => void (s[k] = v) } as unknown as Storage
})

// `vi.mock` is hoisted above the imports: the doubles the factories close over must be hoisted with them.
const m = vi.hoisted(() => ({
  api: {
    create_session: vi.fn(async () => ({ session: 'fresh1', retention: 'durable' })),
    rename_session: vi.fn(async () => null),
  },
  ctx: { notebook_context_set: vi.fn(async () => null) },
  invalidateQueries: vi.fn(),
  chats: [] as { id: string }[],
}))
const { api, ctx, invalidateQueries } = m

vi.mock('@tanstack/react-query', () => ({ useQueryClient: () => ({ invalidateQueries: m.invalidateQueries }) }))
vi.mock('@/lib/rpc', () => ({ useApi: () => m.api }))
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
  expect(api.create_session.mock.calls[0]![0]).toMatchObject({ notebook: 'nb1', retention: 'durable' })
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
})
