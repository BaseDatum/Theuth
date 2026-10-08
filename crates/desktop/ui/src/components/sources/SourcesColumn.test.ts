import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as NbRpc from '@/lib/notebooks-rpc'
import type { NotebookSource } from '@/rpc/bindings/NotebookSource'
import type * as Rpc from '@/lib/rpc'
import { SourcesColumn } from './SourcesColumn'

const fixture: NotebookSource = {
  doc: { id: 'src1', kind: 'source', title: 'ICD portal DR runbook v7', binding: 'notebook', created_ms: 0n, updated_ms: 0n, bytes: 0n } as unknown as NotebookSource['doc'],
  origin: { kind: 'file', name: 'runbook.pdf', sha256: 'x', ext: 'pdf' },
  status: 'completed',
  progress: null,
  error: null,
  attempts: 1,
  fetched_ms: BigInt(Date.now() - 3 * 86_400_000),
  content_sha: 'abc',
  changed_since: null,
  bytes: 1n,
  words: 9000,
  duration_ms: null,
  pages: 48,
  insights: 1,
  meta: null,
}

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children }: { children: ReactNode }) => createElement('a', null, children),
  useNavigate: () => vi.fn(),
}))
vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn() }) }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => ({}) }))
vi.mock('@/lib/notebooks-rpc', async (original) => ({ ...(await original<typeof NbRpc>()), useNotebooksApi: () => ({}) }))
vi.mock('@/components/notebooks/useNotebook', () => ({ useSources: () => ({ data: [fixture], isPending: false, isError: false }) }))

test('the column: header count, the wave-2 bulk row, one card with glyph, title, status and origin line', () => {
  const html = renderToStaticMarkup(createElement(QueryClientProvider, { client: new QueryClient(), children: createElement(SourcesColumn, { notebook: 'nb1' }) as ReactNode }))
  expect(html).toContain('Sources')
  expect(html).toContain('· 1')
  expect(html).toContain('>PDF<')
  expect(html).toContain('ICD portal DR runbook v7')
  expect(html).toContain('ready')
  expect(html).toContain('file · 48 pages · fetched 3 d ago · 1 insight')
  expect(html).toContain('title="wave 2"')
  expect(html).not.toContain('re-fetch')
})
