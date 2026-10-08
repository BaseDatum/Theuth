// 144 §4 (Sky's ruling): the way back to the notebook renders at every width, and an item opened without `?nb`
// lists its notebooks. Static markup; the shell branch (`isPopout() === false`), desktop (`useIsMobile() === false`).
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type { Notebook } from '@/rpc/bindings/Notebook'
import { DocumentPage, useNotebookBack } from './DocumentPage'

const nb = (id: string, title: string): Notebook => ({ doc: { id, title } } as unknown as Notebook)
let members: Notebook[] | undefined = [nb('nb1', 'DR failover'), nb('nb2', 'Vendor review')]

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children, to, params, search, ...rest }: { children?: ReactNode; to: string; params?: Record<string, string>; search?: Record<string, unknown> }) =>
    createElement('a', { ...rest, href: to, 'data-params': JSON.stringify(params ?? {}), 'data-search': JSON.stringify(search ?? {}) }, children),
}))
vi.mock('@/components/editor/Editor', () => ({ Editor: () => null }))
vi.mock('@/components/nav/SectionBand', () => ({ SectionBand: () => null }))
vi.mock('@/components/share/ParticipantStrip', () => ({ ParticipantStrip: () => null }))
vi.mock('@/components/share/ShareDialog', () => ({ ShareAction: () => null }))
vi.mock('@/components/sidebar/SearchTrigger', () => ({ SearchTrigger: () => null }))
vi.mock('@/hooks/use-mobile', () => ({ useIsMobile: () => false }))
vi.mock('@/lib/popout', () => ({ isPopout: () => false }))
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

function Page({ kind, id, nb: from }: { kind: 'note' | 'source'; id: string; nb?: string }) {
  const { back, chip } = useNotebookBack(kind, id, from)
  return createElement(DocumentPage, {
    backTo: '/notes',
    backLabel: 'Back to notes',
    back,
    band: false,
    title: createElement('h2', null, 'A note', chip),
    live: { provider: null, status: { kind: 'connecting' } } as never,
  })
}

const render = (props: { kind: 'note' | 'source'; id: string; nb?: string }) =>
  renderToStaticMarkup(createElement(QueryClientProvider, { client: new QueryClient(), children: createElement(Page, props) }))

test('with ?nb the back control renders at desktop width with the notebook title and lands on the notes tab; no list chevron, no chip', () => {
  const html = render({ kind: 'note', id: 'n1', nb: 'nb2' })
  expect(html).toContain('aria-label="Back to Vendor review"')
  expect(html).toContain('>Back to Vendor review</span>')
  expect(html).not.toContain('md:hidden')
  expect(html).toMatch(/<a href="\/notebooks\/\$id" data-params="{&quot;id&quot;:&quot;nb2&quot;}" data-search="{&quot;tab&quot;:&quot;notes&quot;}"/)
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
