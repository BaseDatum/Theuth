import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { ATTENTION_SIM, SANDBOXES_SIM, SECRET_REQUESTS_SIM } from '@/components/settings/sandbox-queries'
import type * as Router from '@tanstack/react-router'
import type * as Hotkeys from '@/lib/hotkeys'
import type * as Rpc from '@/lib/rpc'
import type * as Sidebar from '@/components/ui/sidebar'
import { AttentionList, Hud } from './Hud'

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
// The shell: a sidebar context (open, desktop) and a browser-width viewport, so `Hud` paints the desktop scene.
vi.mock('@/components/ui/sidebar', async (original) => ({
  ...(await original<typeof Sidebar>()),
  useSidebar: () => ({ open: true, isMobile: false, toggleSidebar: () => undefined }),
}))
vi.mock('@/hooks/use-mobile', () => ({ useIsMobile: () => false }))
vi.mock('@/lib/hotkeys', async (original) => ({ ...(await original<typeof Hotkeys>()), isMacDesktop: () => false, touch: () => false }))

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

test('R2/R3: `/` is the Overview header, the scene and the docked composer — none of the retired sections (Attention cards, In flight, Board pulse, Activity, Spend)', () => {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const html = renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(Hud) as ReactNode }))
  expect(html).toContain('>Overview<')
  expect(html).toMatch(/<svg[^>]*viewBox="0 0 1920 812"/)
  for (const title of ['Attention', 'In flight', 'Board pulse', 'Activity', 'Spend']) expect(html).not.toContain(`<section aria-label="${title}"`)
  expect(html).toMatch(/<textarea[^>]*placeholder="Ask anything"/) // the docked new-chat composer (R3 restored it)
  // Queries are still loading on the first paint: the chrome shows, the inbox count is a dash, nothing is a false zero.
  expect(html).toContain('— NEED YOU')
})
