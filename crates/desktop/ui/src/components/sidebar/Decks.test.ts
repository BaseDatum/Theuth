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

import { deckIdFromPath, DecksMenu, DecksRail } from './Decks'

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

  it('decodes /decks/<id> paths safely and does not throw on malformed % sequences (L2)', () => {
    expect(deckIdFromPath('/chat')).toBeUndefined()
    expect(deckIdFromPath('/decks')).toBe('')
    expect(deckIdFromPath('/decks/deck%20pulse')).toBe('deck pulse')
    expect(() => deckIdFromPath('/decks/%E0%A4%A')).not.toThrow()
    expect(deckIdFromPath('/decks/%E0%A4%A')).toBe('%E0%A4%A')
  })

  it('omits empty <ul>, shows loading/error states, handles invalid timestamps, and wraps ArrowDown/ArrowUp/Home/End focus (Medium #10)', async () => {
    // Empty list omits <ul> and displays loading/error states when applicable
    const emptyHtml = renderToString(
      h(DecksMenu, { rows: [], isPending: true, onNew: () => {}, onPick: () => {} }),
    )
    expect(emptyHtml).not.toContain('<ul')
    expect(emptyHtml).toContain('data-decks-loading')

    const errHtml = renderToString(
      h(DecksMenu, { rows: [], isError: true, errorMessage: 'RPC offline', onNew: () => {}, onPick: () => {} }),
    )
    expect(errHtml).not.toContain('<ul')
    expect(errHtml).toContain('RPC offline')

    // Invalid updated_ms does not throw RangeError
    const badTsRow: DeckSummary = { ...deck(0), updated_ms: Number.NaN }
    expect(() =>
      renderToString(h(DecksMenu, { rows: [badTsRow], onNew: () => {}, onPick: () => {} })),
    ).not.toThrow()

    // Mounted keyboard navigation across [data-rail-item] rows
    await act(async () => {
      root.render(
        h(DecksMenu, {
          rows: [deck(0), deck(1)],
          onNew: () => {},
          onNewExample: () => {},
          onPick: () => {},
        }),
      )
    })
    const menu = el.querySelector<HTMLElement>('[data-decks-menu]')!
    const items = [...menu.querySelectorAll<HTMLElement>('[data-rail-item]')]
    expect(items.length).toBe(5) // New, Example, deck-0, deck-1, All decks

    items[0]!.focus()
    expect(document.activeElement).toBe(items[0])

    await act(async () => {
      items[0]!.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true, cancelable: true }))
    })
    expect(document.activeElement).toBe(items[1])

    await act(async () => {
      items[1]!.dispatchEvent(new KeyboardEvent('keydown', { key: 'End', bubbles: true, cancelable: true }))
    })
    expect(document.activeElement).toBe(items[items.length - 1])

    // ArrowDown from last wraps to first
    await act(async () => {
      items[items.length - 1]!.dispatchEvent(
        new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true, cancelable: true }),
      )
    })
    expect(document.activeElement).toBe(items[0])

    // ArrowUp from first wraps to last
    await act(async () => {
      items[0]!.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true, cancelable: true }))
    })
    expect(document.activeElement).toBe(items[items.length - 1])

    await act(async () => {
      items[items.length - 1]!.dispatchEvent(
        new KeyboardEvent('keydown', { key: 'Home', bubbles: true, cancelable: true }),
      )
    })
    expect(document.activeElement).toBe(items[0])
  })

  it('attaches /decks drag payloads to DecksRail and DecksRow, guards popover while dragging, and preserves DecksMenu/DecksList drag payloads', async () => {
    const { QueryClient, QueryClientProvider } = await import('@tanstack/react-query')
    const { DecksRow } = await import('./Decks')
    const { DecksList } = await import('@/components/decks/DecksList')
    const { SidebarProvider } = await import('@/components/ui/sidebar')
    const { usePanes } = await import('@/lib/panes')
    decksMock.rows = [deck(0)]

    const pointerEv = (type: string, clientX: number, clientY: number) =>
      Object.assign(
        new MouseEvent(type, { bubbles: true, cancelable: true, button: 0, clientX, clientY }),
        { pointerId: 1, pointerType: 'mouse' },
      )

    const startDragFrom = async (target: HTMLElement) => {
      usePanes.setState({ drag: null })
      target.setPointerCapture = () => {}
      target.releasePointerCapture = () => {}
      target.hasPointerCapture = () => false
      await act(async () => {
        target.dispatchEvent(pointerEv('pointerdown', 10, 10))
      })
      await act(async () => {
        target.dispatchEvent(pointerEv('pointermove', 30, 10))
      })
      const payload = usePanes.getState().drag?.payload
      await act(async () => {
        target.dispatchEvent(pointerEv('pointerup', 30, 10))
      })
      usePanes.setState({ drag: null })
      return payload
    }

    // 1) Collapsed DecksRail: click opens popover; drag carries /decks payload and suppresses popover
    await act(async () => {
      root.render(h(DecksRail, { itemClass: 'rail-item', wrap: (t) => t }))
    })
    const railBtn = el.querySelector<HTMLButtonElement>('[data-decks-rail-button]')!
    expect(railBtn.getAttribute('data-drag-row')).toBe('true')
    await act(async () => {
      railBtn.click()
    })
    expect(document.body.querySelector('[data-decks-menu]')).not.toBeNull()
    // Close popover before testing drag.
    await act(async () => {
      railBtn.click()
    })
    expect(document.body.querySelector('[data-decks-menu]')).toBeNull()
    expect(await startDragFrom(railBtn)).toEqual({ kind: 'view', path: '/decks', title: 'Decks' })
    // Trailing click immediately after drag is swallowed by usePressDrag so popover stays closed.
    await act(async () => {
      railBtn.click()
    })
    expect(document.body.querySelector('[data-decks-menu]')).toBeNull()

    // 2) Expanded DecksRow: click opens popover; drag carries /decks payload and suppresses popover
    await act(async () => {
      root.render(h(SidebarProvider, { defaultOpen: true }, h(DecksRow)))
    })
    const rowBtn = el.querySelector<HTMLButtonElement>('[data-decks-row-button]')!
    expect(rowBtn.getAttribute('data-drag-row')).toBe('true')
    await act(async () => {
      rowBtn.click()
    })
    expect(document.body.querySelector('[data-decks-menu]')).not.toBeNull()
    await act(async () => {
      rowBtn.click()
    })
    expect(document.body.querySelector('[data-decks-menu]')).toBeNull()
    expect(await startDragFrom(rowBtn)).toEqual({ kind: 'view', path: '/decks', title: 'Decks' })
    await act(async () => {
      rowBtn.click()
    })
    expect(document.body.querySelector('[data-decks-menu]')).toBeNull()

    // 3) DecksMenu item and DecksList row carry per-deck /decks/<id> drag payload
    await act(async () => {
      root.render(h(DecksMenu, { rows: [deck(0)], onNew: () => {}, onPick: () => {} }))
    })
    const menuDeck = el.querySelector<HTMLElement>('[data-deck="deck-0"]')!
    expect(await startDragFrom(menuDeck)).toEqual({ kind: 'view', path: '/decks/deck-0', title: 'Deck 0' })

    const qc = new QueryClient()
    await act(async () => {
      root.render(
        h(
          QueryClientProvider,
          { client: qc },
          h(SidebarProvider, { defaultOpen: true }, h(DecksList)),
        ),
      )
    })
    const listRow = el.querySelector<HTMLElement>('[data-deck-row="deck-0"]')!
    expect(await startDragFrom(listRow)).toEqual({ kind: 'view', path: '/decks/deck-0', title: 'Deck 0' })
  })
})
