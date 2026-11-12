import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as NbRpc from '@/lib/notebooks-rpc'
import type * as Rpc from '@/lib/rpc'
import { askKeys } from '@/lib/ask-rpc'
import { notebookKeys } from '@/lib/notebooks-rpc'
import type { AskSummary } from "@/rpc/bindings/method.agent.output"
import { NotebooksList } from './NotebooksList'

const env = vi.hoisted(() => ({ search: {} as Record<string, unknown> }))
vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn() }) }))
vi.mock('@tanstack/react-router', () => ({
  Link: ({ children, to, params }: { children?: unknown; to: string; params?: Record<string, string> }) =>
    createElement('a', { href: Object.entries(params ?? {}).reduce((p, [k, v]) => p.replace(`$${k}`, v), to) }, children as never),
  getRouteApi: () => ({ useSearch: () => env.search }),
  useNavigate: () => vi.fn(),
  useRouterState: () => '/notebooks',
}))
vi.mock('@/components/ui/sidebar', () => ({ useSidebar: () => ({ open: true, isMobile: false, toggleSidebar: () => {} }) }))
vi.mock('@/components/sidebar/SearchTrigger', () => ({ SearchTrigger: () => null }))
vi.mock('@/lib/popout', () => ({ isPopout: () => false, canPopout: () => true, openPopout: vi.fn() }))
vi.mock('@/store/connection', () => ({ useConnection: (pick: (s: unknown) => unknown) => pick({ client: { call: vi.fn() }, discovery: { phase: 'legacy' } }) }))
vi.mock('@/store/sessions', () => ({ on: () => () => {} }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => ({ doc_markdown: async () => '' }) }))
vi.mock('@/lib/notebooks-rpc', async (original) => ({ ...(await original<typeof NbRpc>()), useNotebooksApi: () => ({}) }))
vi.mock('@/components/notebooks/AskDialog', () => ({ AskDialog: ({ open }: { open: boolean }) => (open ? createElement('div', { 'data-ask': true }) : null) }))

const ask = (session: string, question: string, scope: string[], done: boolean): AskSummary => ({ session, question, scope, created_ms: Date.now() - 120_000, done })

const render = () => {
  const qc = new QueryClient()
  qc.setQueryData(notebookKeys.list(false), [])
  qc.setQueryData(askKeys.list, [ask('s1', 'What is the RTO?', ['a', 'b'], true), ask('s2', 'Who owns DNS?', ['a'], false)])
  return renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(NotebooksList) }))
}

test('Library › Notebooks carries the Notebooks · Ask sub-tabs; Ask lists past asks with scope count, when and state, each to /ask/$session', () => {
  env.search = {}
  const list = render()
  expect(list).toContain('role="tab"')
  expect(list).toContain('>Ask<')
  expect(list).toContain('No notebooks yet')
  expect(list).not.toContain('What is the RTO?')
  env.search = { view: 'ask' }
  const asks = render()
  expect(asks).toContain('href="/ask/s1"')
  expect(asks).toContain('What is the RTO?')
  expect(asks).toContain('2 notebooks · 2m · done')
  expect(asks).toContain('1 notebook · 2m · running')
  expect(asks).not.toContain('data-ask')
  env.search = { view: 'ask', ask: true }
  expect(render()).toContain('data-ask')
})
