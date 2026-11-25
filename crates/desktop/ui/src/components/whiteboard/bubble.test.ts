import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { Entry } from '@/store/sessions'
import type { Board, Card } from './canvas/model'
import { COLLAPSE_MS, bubbleAnchor, bubbleTimer, lastReply, oneLine } from './reply'
import type { WbTurnInfo } from './wire'

const card = (id: string, state: Card['state'] = 'live'): Card => ({ id, kind: 'page', x: 0, y: 0, w: 180, h: 390, z: 1, title: id, versions: [], state, by: 'owner' })
const board = { cards: { A: card('A'), B: card('B'), Z: card('Z', 'archived') } } as unknown as Pick<Board, 'cards'>
const turn = (cards: string[], over: Partial<WbTurnInfo> = {}): WbTurnInfo => ({ turn: 1, cards, links: [], frames: [], writes: cards.length, undone: false, ended: true, ...over })

describe('bubble placement', () => {
  it('is pinned to the one card the turn touched', () => {
    expect(bubbleAnchor(turn(['A']), board)).toBe('A')
    expect(bubbleAnchor(turn(['A', 'A']), board)).toBe('A')
  })
  it('sits above the pill when the turn touched several cards, none, or only archived ones', () => {
    expect(bubbleAnchor(turn(['A', 'B']), board)).toBeNull()
    expect(bubbleAnchor(turn([]), board)).toBeNull()
    expect(bubbleAnchor(turn(['Z']), board)).toBeNull()
    expect(bubbleAnchor(turn(['gone']), board)).toBeNull()
    expect(bubbleAnchor(null, board)).toBeNull()
  })
  it('an undone turn is not pinned', () => {
    expect(bubbleAnchor(turn(['A'], { undone: true }), board)).toBeNull()
  })
})

describe('bubble collapse', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())
  it('collapses to one line after 6 s; a tap opens it and restarts the clock', () => {
    const seen: boolean[] = []
    const t = bubbleTimer((c) => seen.push(c))
    t.start()
    vi.advanceTimersByTime(COLLAPSE_MS - 1)
    expect(seen).toEqual([false])
    vi.advanceTimersByTime(1)
    expect(seen).toEqual([false, true])
    t.expand()
    vi.advanceTimersByTime(COLLAPSE_MS)
    expect(seen).toEqual([false, true, false, true])
    t.dispose()
  })
})

describe('reply text', () => {
  const e = (kind: 'user' | 'assistant', text: string) => ({ id: text, kind, text, open: false }) as unknown as Entry
  it('is the last assistant text of the latest turn', () => {
    expect(lastReply([e('user', 'q'), e('assistant', 'first'), e('assistant', 'Pushed v2 onto Mock B')])).toBe('Pushed v2 onto Mock B')
  })
  it('is null when the latest row is the user’s', () => {
    expect(lastReply([e('assistant', 'old'), e('user', 'new question')])).toBeNull()
  })
  it('folds to one line', () => {
    expect(oneLine('**Pushed** v2\n\nonto `Mock B`')).toBe('Pushed v2 onto Mock B')
  })
})
