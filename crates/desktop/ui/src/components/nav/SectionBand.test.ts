import { readyConnection } from '@/components/settings/test-engine'
import type * as ConnectionModule from '@/store/connection'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { Contact, Monitor } from 'lucide-react'
import { expect, test, vi } from 'vitest'
import { SectionBand, TabBand } from './SectionBand'

type State = { location: { pathname: string } }
const env = vi.hoisted(() => ({ pathname: '/agents', popout: null as { kind: string } | null, open: true, isMobile: false, mac: false, rail: false }))
vi.mock('@tanstack/react-router', () => ({
  useNavigate: () => () => Promise.resolve(),
  useRouterState: ({ select }: { select: (s: State) => string }) => select({ location: { pathname: env.pathname } }),
}))
vi.mock('@/components/ui/sidebar', () => ({
  useSidebar: () => ({ open: env.open, isMobile: env.isMobile, rail: env.rail, toggleSidebar: () => {} }),
}))
vi.mock('@/components/sidebar/SearchTrigger', () => ({ SearchTrigger: () => null }))
// 215's Library overview strip under the band has its own data; the band's tests are about the band.
vi.mock('@/components/library/Overview', () => ({ LibraryOverview: () => null }))
vi.mock('@/lib/popout', () => ({ isPopout: () => env.popout !== null, get POPOUT() { return env.popout } }))
vi.mock('@/lib/hotkeys', () => ({ isMacDesktop: () => env.mac }))
vi.mock('@/store/connection', async (original) => {
  const m = await original<typeof ConnectionModule>()
  return { ...m, useConnection: readyConnection(m.createConnection()) }
})

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

test('library group: eight tabs (Skills after Knowledge, Sky 2026-11-11; Notebooks after Notes, 144; Whiteboards, 218), Meetings active under /meetings/briefs', () => {
  env.pathname = '/meetings/briefs'
  const html = render('library')
  expect(tabs(html)).toBe(8)
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
  env.popout = { kind: 'note' }
  expect(render('library')).toBe('')
  env.popout = null
})

test('a page window keeps the tabs, its way between the section pages, without a sidebar toggle even when narrow', () => {
  env.popout = { kind: 'page' }
  env.pathname = '/notes'
  env.isMobile = true
  const html = render('library')
  expect(tabs(html)).toBe(8)
  expect(active(html)).toEqual(['Notes'])
  expect(html).not.toContain('Show sidebar')
  env.isMobile = false
  env.popout = null
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

test('beside the collapsed icon rail: no toggle (the rail has its own), and only the traffic lights\' overhang as inset', () => {
  env.pathname = '/notes'
  env.open = false
  env.rail = true
  env.mac = true
  const html = render('library')
  expect(html).not.toContain('aria-label="Show sidebar"')
  expect(html).toContain('pl-6')
  expect(html).not.toContain('pl-20')
  env.open = true
  env.rail = false
  env.mac = false
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

test('narrow bands wrap every tab with a 40px minimum instead of clipping later tabs', () => {
  const html = renderToStaticMarkup(createElement(TabBand, {
    tabs: Array.from({ length: 6 }, (_, i) => ({ id: String(i), label: `Tab ${i}`, icon: Monitor })),
    value: '5',
    onValueChange: () => {},
  }))
  expect(tabs(html)).toBe(6)
  expect(active(html)).toEqual(['Tab 5'])
  expect(html).toContain('max-lg:flex-wrap')
  expect(html).toContain('max-lg:group-data-[orientation=horizontal]/tabs:h-auto')
  expect(html.match(/max-lg:min-w-10/g)).toHaveLength(6)
  expect(html.match(/max-lg:h-\[52px\]/g)).toHaveLength(6)
})
