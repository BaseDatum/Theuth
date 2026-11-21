// 144 §4 (Sky's ruling): the way back to the notebook renders at every width, and an item opened without `?nb`
// lists its notebooks. Static markup; the shell branch (`isPopout() === false`), desktop (`useIsMobile() === false`).
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type { Notebook } from "@/rpc/bindings/method.agent.output"
import { DocumentPage, ownedByMeeting, useNotebookBack } from './DocumentPage'

const nb = (id: string, title: string): Notebook => ({ doc: { id, title } } as unknown as Notebook)
let members: Notebook[] | undefined = [nb('nb1', 'DR failover'), nb('nb2', 'Vendor review')]

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children, to, params, search, ...rest }: { children?: ReactNode; to: string; params?: Record<string, string>; search?: Record<string, unknown> }) =>
    createElement('a', { ...rest, href: to, 'data-params': JSON.stringify(params ?? {}), 'data-search': JSON.stringify(search ?? {}) }, children),
}))
vi.mock('@/components/editor/Editor', () => ({ Editor: () => null }))
vi.mock('@/components/nav/SectionBand', () => ({ SectionBand: () => null }))
let mesh: { self_id: string; self_name: string; peers: unknown[] } = { self_id: 'S', self_name: 'studio-mac', peers: [] }
vi.mock('@/components/tasks/queries', () => ({ useMesh: () => ({ data: mesh }) }))
vi.mock('@/components/share/ParticipantStrip', () => ({ ParticipantStrip: () => null }))
vi.mock('@/components/share/ShareDialog', () => ({ ShareAction: () => null }))
vi.mock('@/components/sidebar/SearchTrigger', () => ({ SearchTrigger: () => null }))
vi.mock('@/hooks/use-mobile', () => ({ useIsMobile: () => false }))
vi.mock('@/lib/popout', () => ({ isSoloPopout: () => false }))
vi.mock('@/lib/hotkeys', () => ({ isMacDesktop: () => false }))
vi.mock('@/lib/nav', () => ({ groupOf: () => 'library' }))
// Radix renders menu content only when open; plain elements show what the chip lists.
vi.mock('@/components/ui/dropdown-menu', () => ({
  DropdownMenu: ({ children }: { children: ReactNode }) => createElement('div', { 'data-menu': true }, children),
  DropdownMenuTrigger: ({ children }: { children: ReactNode }) => children,
  DropdownMenuContent: ({ children }: { children: ReactNode }) => createElement('div', { role: 'menu' }, children),
  DropdownMenuItem: ({ children }: { children: ReactNode }) => createElement('div', { role: 'menuitem' }, children),
}))
vi.mock('@/components/notebooks/useNotebook', () => ({
  useItemNotebooks: () => members,
  useNotebook: (id: string, enabled = true) => ({ data: enabled ? members?.find((n) => n.doc.id === id) : undefined }),
}))

function Page({ kind, id, nb: from, embedded }: { kind: 'note' | 'source'; id: string; nb?: string; embedded?: boolean }) {
  const { back, chip } = useNotebookBack(kind, id, from)
  return createElement(DocumentPage, {
    backTo: '/notes',
    backLabel: 'Back to notes',
    back,
    band: false,
    embedded,
    title: createElement('h2', null, 'A note', chip),
    actions: createElement('button', { 'aria-label': 'Rename' }),
    live: { provider: null, status: { kind: 'connecting' } } as never,
  })
}

const render = (props: { kind: 'note' | 'source'; id: string; nb?: string; embedded?: boolean }) =>
  renderToStaticMarkup(createElement(QueryClientProvider, { client: new QueryClient(), children: createElement(Page, props) }))

test('with ?nb the back control renders at desktop width with the notebook title and lands on the notes tab; no list chevron, no chip', () => {
  const html = render({ kind: 'note', id: 'n1', nb: 'nb2' })
  expect(html).toContain('aria-label="Back to Vendor review"')
  expect(html).toContain('>Back to Vendor review</span>')
  expect(html).not.toContain('md:hidden')
  expect(html).toMatch(/<a [^>]*href="\/notebooks\/\$id"[^>]*data-params="{&quot;id&quot;:&quot;nb2&quot;}"[^>]*data-search="{&quot;tab&quot;:&quot;notes&quot;}"/)
  expect(html).not.toContain('in 2 notebooks')
})

test('without ?nb an item in two notebooks goes back to the first and the chip lists both; a source lands on the sources tab', () => {
  const html = render({ kind: 'source', id: 's1' })
  expect(html).toContain('aria-label="Back to DR failover"')
  expect(html).toContain('in 2 notebooks')
  expect(html).toMatch(/role="menu".*DR failover.*Vendor review/s)
  expect(html.match(/data-search="{&quot;tab&quot;:&quot;sources&quot;}"/g)).toHaveLength(3)
})

test('an item in no notebook keeps the phone-only list chevron and shows no chip', () => {
  members = []
  const html = render({ kind: 'note', id: 'n1' })
  expect(html).toContain('aria-label="Back to notes"')
  expect(html).toContain('md:hidden')
  expect(html).not.toContain('role="menu"')
})

test('156 D1: embedded draws no back link of either kind, keeps the toolbar row with title and actions', () => {
  members = [nb('nb1', 'DR failover')]
  const html = render({ kind: 'note', id: 'n1', nb: 'nb1', embedded: true })
  expect(html).not.toContain('Back to')
  expect(html).not.toContain('href="/notes"')
  expect(html).toContain('>A note<')
  expect(html).toContain('aria-label="Rename"')
})

test('156 D1: a meeting note is owned by its meeting — by kind on /notes/<id>, by the meeting prop before meta lands; a plain note is not', () => {
  expect(ownedByMeeting('meeting_note', undefined)).toBe(true)
  expect(ownedByMeeting(undefined, 'm1')).toBe(true)
  expect(ownedByMeeting('note', undefined)).toBe(false)
  expect(ownedByMeeting(undefined, undefined)).toBe(false)
})

// 184 §16: a lone engine shows nothing new; in a mesh the same page gets the chip and the offline banner.
const stub = { id: 'd1', meta: { kind: 'note', home: 'X' }, server: 'S', dual: null, status: { kind: 'live' }, onEvent: () => () => {}, onStatus: () => () => {} }
const renderLive = () =>
  renderToStaticMarkup(
    createElement(QueryClientProvider, {
      client: new QueryClient(),
      children: createElement(DocumentPage, { backTo: '/notes', backLabel: 'Back', band: false, title: 'A note', live: { provider: stub, status: { kind: 'live' }, error: null } as never }),
    }),
  )

test('184: without peers the bar has no chip and the page no banner', () => {
  const html = renderLive()
  expect(html).not.toContain('On xeus')
  expect(html).not.toContain('On studio-mac')
  expect(html).not.toContain('role="status"')
})

test('184: in a mesh a note served away from its home gets the chip and the banner', () => {
  mesh = { ...mesh, peers: [{ id: 'X', name: 'xeus', reachable: true, last_seen_ms: 0 }] }
  const html = renderLive()
  expect(html).toContain('On xeus · offline')
  expect(html).toContain('xeus is offline')
})
