import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { Contact, Monitor } from 'lucide-react'
import { expect, test, vi } from 'vitest'
import { SectionBand, TabBand } from './SectionBand'

type State = { location: { pathname: string } }
const env = vi.hoisted(() => ({ pathname: '/agents', popped: false, open: true, isMobile: false, mac: false }))
vi.mock('@tanstack/react-router', () => ({
  useNavigate: () => () => Promise.resolve(),
  useRouterState: ({ select }: { select: (s: State) => string }) => select({ location: { pathname: env.pathname } }),
}))
vi.mock('@/components/ui/sidebar', () => ({
  useSidebar: () => ({ open: env.open, isMobile: env.isMobile, toggleSidebar: () => {} }),
}))
vi.mock('@/components/sidebar/SearchTrigger', () => ({ SearchTrigger: () => null }))
vi.mock('@/lib/popout', () => ({ isPopout: () => env.popped }))
vi.mock('@/lib/hotkeys', () => ({ isMacDesktop: () => env.mac }))

const render = (group: 'agents' | 'library') => renderToStaticMarkup(createElement(SectionBand, { group }))
const active = (html: string) => [...html.matchAll(/<button[^>]*data-state="active"[^>]*aria-label="([^"]+)"/g)].map((m) => m[1])
const tabs = (html: string) => html.match(/role="tab"/g)?.length ?? 0

test('agents group: four full-width tabs (Autoresearch after Routines, 140), Tasks active under /tasks/abc, labels hidden below md', () => {
  env.pathname = '/tasks/abc'
  const html = render('agents')
  expect(tabs(html)).toBe(4)
  expect(active(html)).toEqual(['Tasks'])
  expect(html).toContain('title="Autoresearch"')
  expect(html.match(/<span class="hidden md:inline">/g)?.length).toBe(4)
  expect(html).toContain('h-[52px]')
  expect(html).not.toContain('@container')
  expect(html).not.toContain('Show sidebar')
})

test('library group: seven tabs (Skills after Knowledge, Sky 2026-11-11; Notebooks after Notes, 144), Meetings active under /meetings/briefs', () => {
  env.pathname = '/meetings/briefs'
  const html = render('library')
  expect(tabs(html)).toBe(7)
  expect(active(html)).toEqual(['Meetings'])
  expect(html).toContain('title="Artifacts"')
  expect(html).not.toContain('title="Autoresearch"')
  expect(html).not.toContain('title="Tools"')
})

test('no tab active on a foreign path', () => {
  env.pathname = '/settings/general'
  expect(active(render('library'))).toEqual([])
})

test('a popped window renders nothing', () => {
  env.popped = true
  expect(render('library')).toBe('')
  env.popped = false
})

test('the sidebar toggle shows on a phone or when the rail is closed', () => {
  env.pathname = '/notes'
  env.isMobile = true
  expect(render('library')).toContain('aria-label="Show sidebar"')
  env.isMobile = false
  env.open = false
  expect(render('library')).toContain('aria-label="Show sidebar"')
  env.open = true
  expect(render('library')).not.toContain('aria-label="Show sidebar"')
})

test('188: a band of any tabs marks its value active; beside a column it leaves the traffic-light inset to the column', () => {
  const band = (inset?: boolean) =>
    renderToStaticMarkup(
      createElement(TabBand, {
        tabs: [
          { id: 'machine', label: 'This machine', icon: Monitor },
          { id: 'contacts', label: 'Contacts', icon: Contact },
        ],
        value: 'contacts',
        onValueChange: () => {},
        inset,
      }),
    )
  env.mac = true
  env.open = false
  expect(tabs(band())).toBe(2)
  expect(active(band())).toEqual(['Contacts'])
  expect(band()).toContain('pl-20')
  expect(band(false)).not.toContain('pl-20')
  // The rail is closed: the toggle still shows, so the sidebar can come back.
  expect(band(false)).toContain('aria-label="Show sidebar"')
  env.mac = false
  env.open = true
})
