import { createElement } from 'react'
import { renderToString } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { Board, Card, Frame, Link, Mark } from './canvas/model'
import { OutlineView, outlineText } from './OutlineView'

const v = (t: Card['kind']) => ({ source: { t: 'page', session: 's', call_id: 'c' }, at: 1, by: 'owner' }) as unknown as Card['versions'][number] & { _?: typeof t }
const card = (id: string, kind: Card['kind'], title: string, x: number, y: number, over: Partial<Card> = {}): Card => ({
  id, kind, title, x, y, w: 180, h: 390, z: 1, versions: [v(kind)], state: 'live', by: 'owner', ...over,
})
const frame: Frame = { id: 'F1', title: 'Variants', x: 36, y: 48, w: 610, h: 424, colour: 'neutral', preset: 'phone', by: 'owner' }
const cards: Record<string, Card> = {
  C1: card('C1', 'image', 'Today', 50, 80, { frame: 'F1' }),
  C2: card('C2', 'page', 'Mock A — grouped', 250, 80, { frame: 'F1', state: 'dimmed', tag: 'header kept; grouping rejected' }),
  C3: card('C3', 'page', 'Mock B — flat list', 450, 80, { frame: 'F1', versions: [v('page'), v('page')] }),
  C4: card('C4', 'sticky', 'Requirements', 50, 494, { w: 220, h: 150 }),
  C5: card('C5', 'diagram', 'Item lifecycle', 330, 494, { w: 250, h: 170 }),
  C6: card('C6', 'page', 'Old idea', 900, 900, { state: 'archived' }),
}
const links: Record<string, Link> = { L1: { id: 'L1', from: 'C4', to: 'C5', label: 'drives', style: 'solid', arrow: 'end', by: 'owner' } }
const marks: Record<string, Mark> = { M4: { id: 'M4', strokes: [], card: 'C3', ver: 2, bb: [0, 0, 1, 1], state: 'sent', region: { kind: 'page', label: 'tab bar, item 3' }, hw: 'no third tab', sent_at: 1, by: 'owner' } }
const board: Board = {
  meta: { v: 1, created: 0, bg: 'dots' },
  cards, links, frames: { F1: frame }, ink: {}, marks,
  aliases: { C1: 'c1', C2: 'c2', C3: 'c3', C4: 'c4', C5: 'c5', C6: 'c6', L1: 'l1', F1: 'f1', M4: 'm4' },
  markOf: {},
}
const noop = () => {}
const html = renderToString(createElement(OutlineView, { board, title: 'Attention inbox', a: { onOpen: noop, onRestore: noop, onPin: noop } }))
const text = html.replace(/<[^>]+>/g, ' ').replace(/&#x27;|&quot;|&amp;/g, (m) => ({ '&#x27;': "'", '&quot;': '"', '&amp;': '&' })[m] ?? m).replace(/\s+/g, ' ')

describe('OutlineView', () => {
  it('lists frames, then rows, then cards in reading order, then loose cards, links and marks', () => {
    const at = (s: string) => {
      const i = text.indexOf(s)
      expect(i, s).toBeGreaterThanOrEqual(0)
      return i
    }
    const order = ['Variants', 'Today', 'Mock A — grouped', 'Mock B — flat list', 'Outside frames', 'Requirements', 'Item lifecycle', 'Links', 'Marks']
    const pos = order.map(at)
    expect([...pos].sort((a, b) => a - b)).toEqual(pos)
  })

  it('labels each card with alias, kind, version and state', () => {
    expect(text).toContain('c3 · Mock B — flat list · page · version 2 of 2')
    expect(text).toContain('c2 · Mock A — grouped · page · dimmed — header kept; grouping rejected')
    expect(html).toContain('aria-label="Frame Variants"')
    expect(html).toContain('aria-label="Row 1"')
  })

  it('shows links and open marks with their captions; archived cards only under Archived', () => {
    expect(text).toContain('Requirements → Item lifecycle')
    expect(text).toContain('m4 · on c3 · sent · page says: tab bar, item 3 · handwriting: no third tab')
    expect(text.indexOf('Old idea')).toBeGreaterThan(text.indexOf('Archived'))
  })

  it('never says "board" without "white"', () => {
    for (const s of [html, outlineText(board, 'Attention inbox')]) expect(s.replace(/whiteboard/gi, '')).not.toMatch(/board/i)
  })

  it('an empty whiteboard says so', () => {
    const empty = renderToString(createElement(OutlineView, { board: { ...board, cards: {}, links: {}, frames: {}, marks: {} }, title: 'New', a: { onOpen: noop } }))
    expect(empty).toContain('This whiteboard has no cards yet')
  })
})

describe('outlineText', () => {
  it('copies the same order as text', () => {
    expect(outlineText(board, 'Attention inbox').split('\n').slice(0, 4)).toEqual(['# Attention inbox', '## Variants', '  - c1 · Today · image', '  - c2 · Mock A — grouped · page · dimmed — header kept; grouping rejected'])
  })
})
