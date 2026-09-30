import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { ATTENTION_SIM, SANDBOXES_SIM, SECRET_REQUESTS_SIM } from '@/components/settings/sandbox-queries'
import type * as Router from '@tanstack/react-router'
import type * as Rpc from '@/lib/rpc'
import { AttentionList } from './Hud'

vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({
    sandboxes_list: async () => SANDBOXES_SIM,
    secret_requests_list: async () => SECRET_REQUESTS_SIM,
  }),
}))
// The first paint (rows not loaded yet) links the item to its session; a `Link` needs a router, a plain anchor does not.
vi.mock('@tanstack/react-router', async (original) => ({
  ...(await original<typeof Router>()),
  Link: ({ children }: { children: ReactNode }) => createElement('a', null, children),
}))

async function render(): Promise<string> {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const items = ATTENTION_SIM.filter((i) => i.kind === 'secret')
  const paint = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(AttentionList, { items }) as ReactNode }))
  for (let pass = 0; pass < 2; pass++) {
    paint()
    for (const query of qc.getQueryCache().findAll()) await qc.fetchQuery({ ...query.options, queryKey: query.queryKey })
  }
  return paint()
}

test('secret inbox items render the secret_requests row: title, one masked input per field, the VM name as origin', async () => {
  const html = await render()
  // The form raised on this host: its title and both fields, the password masked.
  expect(html).toContain('Postgres login')
  expect(html).toMatch(/<input[^>]*type="text"[^>]*aria-label="value of PGUSER"/)
  expect(html).toMatch(/<input[^>]*type="password"[^>]*aria-label="value of PGPASSWORD"/)
  // The single field raised on `lab`'s engine: the VM's name is its origin; the field's label is the note.
  expect(html).toContain('· on lab')
  expect(html).toContain('a fine-grained token with repo scope')
  expect(html).toContain('adds v2')
  // No card falls back to "answer it in the session".
  expect(html).not.toContain('answer it in the session')
})
