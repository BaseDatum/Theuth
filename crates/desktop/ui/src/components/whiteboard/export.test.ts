import { describe, expect, it } from 'vitest'
import type { Board, Card } from './canvas/model'
import { briefMarkdown } from './export'

const DOC = '01a2f3c4-5b6d-7e8f-9a0b-1c2d3e4f5a6b'
const card = (id: string, title: string, x: number, y: number, over: Partial<Card> = {}): Card => ({
  id, kind: 'page', title, x, y, w: 180, h: 390, z: 1, versions: [{ source: { t: 'page', session: 's', call_id: 'c' }, at: 1, by: 'owner' }], state: 'live', by: 'owner', ...over,
})
const board: Board = {
  meta: { v: 1, created: 0, bg: 'dots' },
  cards: {
    A: card('A', 'Mock [A]', 50, 80, { frame: 'F', state: 'dimmed', tag: 'grouping rejected' }),
    B: card('B', 'Mock B', 250, 80, { frame: 'F' }),
    S: card('S', 'Requirements', 50, 600, { kind: 'sticky', versions: [{ source: { t: 'sticky', text: 'one thumb\nnever lose' }, at: 1, by: 'owner' }] }),
    X: card('X', 'Gone', 900, 900, { state: 'archived' }),
  },
  links: { L: { id: 'L', from: 'S', to: 'B', label: 'drives', style: 'solid', arrow: 'end', by: 'owner' } },
  frames: { F: { id: 'F', title: 'Variants', x: 0, y: 0, w: 600, h: 500, colour: 'neutral', by: 'owner' } },
  ink: {},
  marks: { M: { id: 'M', strokes: [], card: 'B', bb: [0, 0, 1, 1], state: 'pinned', region: { kind: 'page', label: 'tab bar' }, sent_at: 1, by: 'owner' } },
  aliases: {},
  markOf: {},
}

describe('briefMarkdown', () => {
  it('is the outline as a note: frames, loose cards, links, pinned marks; every card links back', () => {
    expect(briefMarkdown(board, 'Inbox', DOC)).toBe(
      [
        '# Inbox',
        '',
        `From [the whiteboard](theuth://whiteboards/${DOC}).`,
        '',
        '## Variants',
        '',
        `- [Mock \\[A\\]](theuth://whiteboards/${DOC}#A) · page · set aside — grouping rejected`,
        `- [Mock B](theuth://whiteboards/${DOC}#B) · page`,
        '',
        '## Outside frames',
        '',
        `- [Requirements](theuth://whiteboards/${DOC}#S) · sticky\n  > one thumb\n  > never lose`,
        '',
        '## Links',
        '',
        '- Requirements → Mock B (drives)',
        '',
        '## Pinned marks',
        '',
        '- on Mock B — page says: tab bar',
        '',
      ].join('\n'),
    )
  })
})
