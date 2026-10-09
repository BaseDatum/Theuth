import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as NbRpc from '@/lib/notebooks-rpc'
import type { ContextEstimate } from '@/rpc/bindings/ContextEstimate'
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
const second: NotebookSource = { ...fixture, doc: { ...fixture.doc, id: 'src2', title: 'Vendor SLA' }, insights: 0 }

let estimate: ContextEstimate | undefined = {
  items: [
    { id: 'src1', kind: 'source', title: 'ICD portal DR runbook v7', level: 'summary', pinned: false, tokens: 3400, status: 'available' },
    { id: 'src2', kind: 'source', title: 'Vendor SLA', level: 'full', pinned: true, tokens: 9000, status: 'truncated' },
  ],
  total_tokens: 12_400,
  budget_tokens: 40_000,
  omitted: 0,
  empty: false,
}

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children }: { children: ReactNode }) => createElement('a', null, children),
  useNavigate: () => vi.fn(),
}))
vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn() }) }))
vi.mock('@/hooks/use-mobile', () => ({ useIsMobile: () => false }))
vi.mock('@/store/connection', () => ({ useConnection: (sel: (s: { client: unknown }) => unknown) => sel({ client: {} }) }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => ({}) }))
vi.mock('@/lib/notebooks-rpc', async (original) => ({ ...(await original<typeof NbRpc>()), useNotebooksApi: () => ({}), useContextApi: () => ({}) }))
vi.mock('@/components/notebooks/NotebookContext', () => ({ useNotebookSession: () => 'sess1' }))
vi.mock('@/components/notebooks/useNotebook', () => ({
  useSources: () => ({ data: [fixture, second], isPending: false, isError: false }),
  useContextEstimate: () => ({ data: estimate }),
}))

const render = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: new QueryClient(), children: createElement(SourcesColumn, { notebook: 'nb1' }) as ReactNode }))

test('the column: header count, the live bulk row with the sources\' ~tokens, cards with glyph, title, status and origin line', () => {
  const html = render()
  expect(html).toContain('Sources')
  expect(html).toContain('· 2')
  expect(html).toContain('>PDF<')
  expect(html).toContain('ICD portal DR runbook v7')
  expect(html).toContain('ready')
  expect(html).toContain('file · 48 pages · fetched 3 d ago · 1 insight')
  expect(html).toContain('all sources:')
  expect(html).toContain('~12.4k tokens')
  expect(html).not.toContain('title="wave 2"')
  expect(html).not.toContain('re-fetch')
})

test('wave 2 §5.1: each card gets its dial from the estimate — the effective level selected, a dot only when pinned, truncated flagged; cards carry item anchors', () => {
  const html = render()
  expect(html).toContain('id="item-src1"')
  expect(html).toContain('id="item-src2"')
  // src1: summary by the default rule — selected, no dot, no ×.
  expect(html).toMatch(/aria-checked="true" class="[^"]*">summary/)
  // src2: pinned full — selected with the dot and the "back to the default rule" ×.
  expect(html).toMatch(/aria-checked="true" data-pinned="true"[^>]*>full/)
  expect(html).toContain('aria-label="Back to the default rule"')
  expect(html).toContain('>truncated<')
})

test('no estimate yet: the bulk row shows ~— tokens and the dials have no selection', () => {
  estimate = undefined
  const html = render()
  expect(html).toContain('~— tokens')
  expect(html).not.toContain('aria-checked="true"')
})
