import '../settings/test-dom'
import { act, createElement as h, type ReactNode } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { renderToString } from 'react-dom/server'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { DeckSummary } from '@/components/decks/types'
import { POPOVER_CAP } from './Rail'

const decksMock = vi.hoisted(() => ({
  rows: [] as DeckSummary[],
  mutate: vi.fn(),
}))

vi.mock('@tanstack/react-router', async (original) => ({
  ...(await original<object>()),
  Link: ({ to, params, children, onClick: _o, ...rest }: { to: string; params?: { id: string }; children: ReactNode; onClick?: unknown }) =>
    h('a', { href: params ? to.replace('$id', params.id) : to, ...rest }, children),
  useNavigate: () => vi.fn(),
  useRouterState: ({ select }: { select: (s: { location: { pathname: string } }) => unknown }) =>
    select({ location: { pathname: '/decks' } }),
}))

vi.mock('@/components/decks/useDeckLive', () => ({
  useDecksList: () => ({ data: decksMock.rows, isPending: false, isError: false }),
  useCreateDeck: () => ({ mutate: decksMock.mutate, isPending: false }),
}))

import { DecksMenu, DecksRail } from './Decks'

const deck = (i: number): DeckSummary => ({
  id: `deck-${i}`,
  title: `Deck ${i}`,
  description: `Description ${i}`,
  section_count: 3,
  session_id: `sess-${i}`,
  state: i === 0 ? 'streaming' : 'stopped',
  stale: false,
  active_rev: 1,
  pending_rev: null,
  updated_ms: 1_700_000_000_000 - i * 1000,
})

describe('Decks sidebar rail & popover', () => {
  let el: HTMLDivElement
  let root: Root

  beforeEach(() => {
    el = document.createElement('div')
    document.body.append(el)
    root = createRoot(el)
    decksMock.rows = []
    decksMock.mutate.mockClear()
  })

  afterEach(() => {
    act(() => root.unmount())
    el.remove()
    document.body.innerHTML = ''
  })

  it('renders EXACTLY ONE collapsed rail button regardless of how many Decks exist', async () => {
    for (const count of [0, 1, 5, POPOVER_CAP + 12]) {
      decksMock.rows = Array.from({ length: count }, (_, i) => deck(i))
      await act(async () => {
        root.render(h(DecksRail, { itemClass: 'rail-item', wrap: (t) => t }))
      })
      const buttons = el.querySelectorAll('[data-decks-rail-button]')
      expect(buttons).toHaveLength(1)
      expect(buttons[0]!.getAttribute('aria-label')).toBe('Decks')
      // Never renders per-deck buttons directly in the collapsed rail.
      expect(el.querySelectorAll('[data-deck]')).toHaveLength(0)
    }
  })

  it('popover lists recent decks capped at POPOVER_CAP plus New deck, Workspace Pulse (Example), and All decks', () => {
    const rows = Array.from({ length: POPOVER_CAP + 4 }, (_, i) => deck(i))
    const html = renderToString(
      h(DecksMenu, {
        rows,
        active: 'deck-0',
        onNew: () => {},
        onNewExample: () => {},
        onPick: () => {},
      }),
    )
    expect(html.match(/data-deck="/g)).toHaveLength(POPOVER_CAP)
    expect(html).toContain('href="/decks/deck-0"')
    expect(html).not.toContain(`/decks/deck-${POPOVER_CAP}"`)
    expect(html).toContain('New deck')
    expect(html).toContain('Workspace Pulse (Example)')
    expect(html).toContain('href="/decks"')
    expect(html.replace(/<!-- -->/g, '')).toContain(`All decks (${POPOVER_CAP + 4})`)
  })

  it('empty list still offers New deck, Workspace Pulse (Example), and All decks', () => {
    const html = renderToString(
      h(DecksMenu, {
        rows: [],
        onNew: () => {},
        onNewExample: () => {},
        onPick: () => {},
      }),
    )
    expect(html).toContain('New deck')
    expect(html).toContain('Workspace Pulse (Example)')
    expect(html).not.toContain('data-deck="')
    expect(html).toContain('All decks')
  })
})
