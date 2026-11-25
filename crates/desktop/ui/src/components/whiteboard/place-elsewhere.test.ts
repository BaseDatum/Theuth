import { describe, expect, it } from 'vitest'
import type { Artifact } from '@/rpc/bindings/event.artifact_2eshown.output'
import type { Card } from './canvas/model'
import { cardFromArtifact, spotOn } from './place-elsewhere'

const art = (kind: Artifact['kind']) => ({ kind, call_id: 'toolu_1', session: 'S', path: '/a/b.txt', title: 'Thing', name: 'b.txt' }) as Artifact
const card = (x: number, y: number, w: number, state: Card['state'] = 'live') => ({ x, y, w, h: 100, state }) as Card

describe('Add to whiteboard', () => {
  it('lands right of the right-most live card, (0, 0) when empty', () => {
    expect(spotOn({ cards: {} })).toEqual({ x: 0, y: 0 })
    expect(spotOn({ cards: { a: card(0, 10, 200), b: card(300, 40, 100), z: card(2000, 0, 10, 'archived') } })).toEqual({ x: 448, y: 40 })
  })

  it('maps artifact kinds to cards; text is read once and capped', async () => {
    const read = async () => 'x'.repeat(20_000)
    expect(await cardFromArtifact(art('html'), read)).toEqual({ kind: 'page', title: 'Thing', source: { t: 'page', session: 'S', call_id: 'toolu_1' } })
    expect((await cardFromArtifact(art('image'), read))?.kind).toBe('image')
    const ex = await cardFromArtifact(art('text'), read)
    expect(ex?.kind).toBe('excerpt')
    expect(ex?.source.t === 'excerpt' && ex.source.text.length).toBe(16_000)
    expect((await cardFromArtifact(art('mermaid'), async () => 'graph TD'))?.source).toEqual({ t: 'diagram', lang: 'mermaid', code: 'graph TD' })
    expect(await cardFromArtifact(art('pdf'), read)).toBeNull()
  })
})
