import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { ATTENTION_SIM, SANDBOXES_SIM, SECRET_REQUESTS_SIM } from '@/components/settings/sandbox-queries'
import type * as Router from '@tanstack/react-router'
import type * as Hotkeys from '@/lib/hotkeys'
import type * as Rpc from '@/lib/rpc'
import { queryKeys } from '@/lib/rpc'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import type * as Sidebar from '@/components/ui/sidebar'
import { AttentionList, Hud } from './Hud'

const rpc = vi.hoisted(() => ({ desk_open: vi.fn(async (_name: string) => 'desk-9') }))
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({
    sandboxes_list: async () => SANDBOXES_SIM,
    secret_requests_list: async () => SECRET_REQUESTS_SIM,
    desk_open: rpc.desk_open,
  }),
}))
// The first paint (rows not loaded yet) links the item to its session; a `Link` needs a router, a plain anchor does not.
const navigate = vi.hoisted(() => vi.fn())
vi.mock('@tanstack/react-router', async (original) => ({
  ...(await original<typeof Router>()),
  Link: ({ children }: { children: ReactNode }) => createElement('a', null, children),
  useNavigate: () => navigate,
}))
// The shell: a sidebar context (open, desktop) and a browser-width viewport, so `Hud` paints the desktop scene.
vi.mock('@/components/ui/sidebar', async (original) => ({
  ...(await original<typeof Sidebar>()),
  useSidebar: () => ({ open: true, isMobile: false, toggleSidebar: () => undefined }),
}))
vi.mock('@/hooks/use-mobile', () => ({ useIsMobile: () => false }))
vi.mock('@/lib/hotkeys', async (original) => ({ ...(await original<typeof Hotkeys>()), isMacDesktop: () => false }))
// The real composer reads `window` (keyboard inset, touch) at render and cannot paint in node; a marker stands in and keeps the
// last props so the desk binding (`session`, `resolveSession`, `onSent`) can be exercised.
type ComposerProps = { session: string | null; resolveSession?: () => Promise<string>; onSent?: (session: string) => void }
const composer = vi.hoisted(() => ({ props: null as ComposerProps | null }))
vi.mock('@/components/chat/Composer', () => ({
  Composer: (p: ComposerProps) => {
    composer.props = p
    return createElement('textarea', { 'data-composer': p.session ?? 'new' })
  },
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

const paint = (qc: QueryClient) => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(Hud) as ReactNode }))
const client = () => new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })

test('R2/R3: `/` is the Overview header, the scene and the docked composer — none of the retired sections (Attention cards, In flight, Board pulse, Activity, Spend)', () => {
  const html = paint(client())
  expect(html).toContain('>Overview<')
  expect(html).toMatch(/<svg[^>]*viewBox="0 0 1920 812"/)
  for (const title of ['Attention', 'In flight', 'Board pulse', 'Activity', 'Spend']) expect(html).not.toContain(`<section aria-label="${title}"`)
  // The docked composer (R3 restored it) sits below the scrolling scene, not inside it.
  expect(html).toMatch(/<textarea data-composer=/)
  expect(html.indexOf('viewBox="0 0 1920 812"')).toBeLessThan(html.indexOf('data-composer'))
  // Queries are still loading on the first paint: the chrome shows, the inbox count is a dash, nothing is a false zero.
  expect(html).toContain('— NEED YOU')
})

test("R6: the composer talks to theuth's desk — resolved by `desk_open` on the first send when unknown, bound directly when the sessions list has the desk; a send opens /c/<desk>", async () => {
  // Unknown desk: the composer is unbound but carries a resolver; the first send opens the desk once.
  rpc.desk_open.mockClear()
  navigate.mockClear()
  expect(paint(client())).toContain('data-composer="new"')
  expect(rpc.desk_open).not.toHaveBeenCalled()
  expect(composer.props?.session).toBeNull()
  await expect(composer.props!.resolveSession!()).resolves.toBe('desk-9')
  expect(rpc.desk_open).toHaveBeenCalledWith('theuth')
  // Known desk (a `desk: 'theuth'` row in the sessions list): bound without the RPC; a successful send navigates to it.
  const qc = client()
  const row: SessionSummary = { id: 'desk-1', created_at: '2026-10-07T09:00:00Z', updated_at: '2026-10-07T12:00:00Z', title: "theuth's desk", tags: [], model: 'm', parent: null, name: null, effort: null, agent: 'theuth', cwd: null, desk: 'theuth', pinned: false, archived: false, project: null, notebook: null, retention: 'durable', running: false, owner: 'here', epoch: 1, here: true, memory: 'epochs' }
  qc.setQueryData(queryKeys.sessions, [row])
  rpc.desk_open.mockClear()
  expect(paint(qc)).toContain('data-composer="desk-1"')
  expect(rpc.desk_open).not.toHaveBeenCalled()
  expect(composer.props?.resolveSession).toBeUndefined()
  composer.props!.onSent!('desk-1')
  expect(navigate).toHaveBeenCalledWith({ to: '/c/$session', params: { session: 'desk-1' } })
})
