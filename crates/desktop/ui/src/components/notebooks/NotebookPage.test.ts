import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Popout from '@/lib/popout'
import { NotebookPage } from './NotebookPage'

const env = vi.hoisted(() => ({ popped: false }))
vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn() }) }))
vi.mock('@tanstack/react-router', () => ({
  Link: ({ children, ...p }: { children?: unknown; 'aria-label'?: string }) => createElement('a', p, children as never),
  getRouteApi: () => ({ useParams: () => ({ id: 'nb1' }), useSearch: () => ({}) }),
  useNavigate: () => vi.fn(),
  useRouterState: () => '/notebooks/nb1',
}))
vi.mock('@/lib/popout', async (original) => ({ ...(await original<typeof Popout>()), isPopout: () => env.popped }))
vi.mock('@/hooks/use-mobile', () => ({ useIsMobile: () => false }))
vi.mock('@/components/ui/sidebar', () => ({ useSidebar: () => ({ open: true, isMobile: false, toggleSidebar: () => {} }) }))
vi.mock('@/components/sidebar/SearchTrigger', () => ({ SearchTrigger: () => null }))
vi.mock('@/components/editor/useDocument', () => ({ useDocument: () => ({ provider: null, status: { kind: 'connecting' } }) }))
vi.mock('@/components/notes/useNotes', () => ({ useNotes: () => ({ data: [] }) }))
vi.mock('@/components/notebooks/useNotebook', () => ({
  useNotebook: () => ({ data: { doc: { id: 'nb1', title: 'DR failover' }, project: null, sources: 2 } }),
  useNotebookItems: () => ({ data: [] }),
  useDescription: () => ({ data: '' }),
  firstLine: () => '',
}))
vi.mock('@/components/notebooks/ChatColumn', () => ({ ChatColumn: () => createElement('div', { 'data-col': 'chat' }) }))
vi.mock('@/components/notebooks/NotesColumn', () => ({ NotesColumn: () => createElement('div', { 'data-col': 'notes' }) }))
vi.mock('@/components/sources/SourcesColumn', () => ({ SourcesColumn: () => createElement('div', { 'data-col': 'sources' }) }))
vi.mock('@/components/notebooks/NotebookHeaderActions', () => ({ NotebookHeaderActions: () => createElement('div', { 'data-actions': true }) }))
vi.mock('@/components/share/ShareDialog', () => ({ ShareAction: () => null }))
vi.mock('@/lib/notebooks-rpc', () => ({ useNotebooksApi: () => ({}), notebookKeys: { all: ['notebooks'] } }))

const render = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: new QueryClient(), children: createElement(NotebookPage) }))

test('the shell shows the Library band and the phone back control; a popped window (Sky\'s ruling) shows neither, and all three columns still render', () => {
  const shell = render()
  expect(shell).toContain('aria-label="Notebooks"')
  expect(shell).toContain('aria-label="Back to notebooks"')
  expect(shell).toContain('data-actions')
  env.popped = true
  const popped = render()
  expect(popped).not.toContain('aria-label="Notebooks"')
  expect(popped).not.toContain('aria-label="Back to notebooks"')
  for (const col of ['sources', 'chat', 'notes']) expect(popped).toContain(`data-col="${col}"`)
  expect(popped).toContain('data-actions')
  env.popped = false
})
