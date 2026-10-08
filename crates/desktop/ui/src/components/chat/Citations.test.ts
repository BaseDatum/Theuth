import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as NotebooksRpc from '@/lib/notebooks-rpc'
import { notebookKeys } from '@/lib/notebooks-rpc'
import type { Citation } from '@/lib/notebooks-types'
import { CitationLink, CitationsProvider, References, citationCount } from './Citations'

const HREF = 'theuth://sources/s1#b12'
const citation: Citation = { href: HREF, kind: 'sources', id: 's1', locator: '#b12', sha8: null, status: 'moved', available: true, title: 'ICD portal DR runbook v7', text: null }

vi.mock('@tanstack/react-router', () => ({ useRouter: () => undefined }))
vi.mock('@/store/connection', () => ({ useConnection: (sel: (s: { client: unknown }) => unknown) => sel({ client: {} }) }))
vi.mock('@/lib/notebooks-rpc', async (original) => ({
  ...(await original<typeof NotebooksRpc>()),
  notebooksApi: () => ({ cite_resolve: async () => [citation] }),
}))

const render = (text: string, resolved: Citation[] | null, ...children: ReactNode[]) => {
  const qc = new QueryClient()
  if (resolved) qc.setQueryData(notebookKeys.cite([HREF]), resolved)
  return renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(CitationsProvider, { text, children }) }))
}

test('a plain https link stays an ordinary anchor; a theuth link becomes chip 1', () => {
  const plain = renderToStaticMarkup(createElement(CitationLink, { href: 'https://example.com', children: 'site' }))
  expect(plain).toBe('<a href="https://example.com">site</a>')
  expect(plain).not.toContain('theuth-cite')
  const html = render(`see [runbook](${HREF})`, null, createElement(CitationLink, { href: HREF, children: 'runbook' }))
  expect(html).toMatch(/<a href="theuth:\/\/sources\/s1#b12" class="theuth-cite" data-kind="sources" title="sources · #b12"[^>]*>1<\/a>/)
})

test('References lists nothing without citations, the resolved title and drift word with them', () => {
  expect(render('no links here', null, createElement(References))).toBe('')
  const html = render(`see [runbook](${HREF})`, [citation], createElement(References))
  expect(html).toContain('ICD portal DR runbook v7')
  expect(html).toContain('#b12')
  expect(html).toContain('>moved<')
  expect(html).not.toContain('unavailable')
  const gone = render(`see [runbook](${HREF})`, [{ ...citation, status: 'missing', available: false }], createElement(References))
  expect(gone).toContain('unavailable — ICD portal DR runbook v7')
  expect(gone).toContain('data-dead="true"')
})

test('citationCount counts distinct valid theuth links', () => {
  expect(citationCount(`[a](${HREF}) [b](${HREF}) [c](theuth://notes/n1) [d](https://x)`)).toBe(2)
})
