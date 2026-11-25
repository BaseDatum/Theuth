import { describe, expect, it } from 'vitest'
import { mkCard } from './fixtures/testing'
import { assignLod, emptyLodState, naturalLod, type LodState } from './lod'
import type { Card } from './model'

const VIEW: [number, number, number, number] = [0, 0, 1000, 800]
const snap = { v: 1, at: 0, theme: 'light' as const }
const page = (id: string, x: number, extra: Partial<Card> = {}) => mkCard(id, x, 0, 300, 600, { kind: 'page', ...extra })

describe('LOD (§8.5)', () => {
  it('240 px on screen → live; below → snapshot when one exists, else chip; under 60 px → chip', () => {
    const c = mkCard('c', 0, 0, 300, 200, { kind: 'diagram' })
    expect(naturalLod(c, 0.8, VIEW)).toBe('live') // 240 px
    expect(naturalLod(c, 0.79, VIEW)).toBe('chip')
    expect(naturalLod({ ...c, snap }, 0.79, VIEW)).toBe('snapshot')
    expect(naturalLod({ ...c, snap }, 0.19, VIEW)).toBe('chip') // 57 px
    expect(naturalLod({ ...c, snap }, 0.2, VIEW)).toBe('snapshot') // 60 px
  })

  it('live only within the view ± one screen; web is never live', () => {
    expect(naturalLod(mkCard('n', 1999, 0, 300, 200), 1, VIEW)).toBe('live')
    expect(naturalLod(mkCard('f', 2001, 0, 300, 200), 1, VIEW)).toBe('chip')
    expect(naturalLod(mkCard('w', 0, 0, 300, 200, { kind: 'web', snap }), 1, VIEW)).toBe('snapshot')
  })

  it('budget 4 (iPad) and 8 (desktop): biggest first, the rest fall back', () => {
    const cards = Array.from({ length: 10 }, (_, i) => page(`p${i}`, i * 50, { w: 300 + i, snap }))
    for (const budget of [4, 8]) {
      const { lod } = assignLod(cards, { zoom: 1, view: VIEW, budget, entered: null, now: 0, state: emptyLodState() })
      const live = cards.filter((c) => lod[c.id] === 'live').map((c) => c.id)
      expect(live).toHaveLength(budget)
      expect(live).toContain('p9')
      expect(cards.filter((c) => lod[c.id] === 'snapshot')).toHaveLength(10 - budget)
    }
    // Stickies and other cheap kinds are not budgeted.
    const cheap = Array.from({ length: 12 }, (_, i) => mkCard(`s${i}`, i * 10, 0, 300, 200))
    const { lod } = assignLod(cheap, { zoom: 1, view: VIEW, budget: 4, entered: null, now: 0, state: emptyLodState() })
    expect(Object.values(lod).every((l) => l === 'live')).toBe(true)
  })

  it('30 s demotion: a live page nobody entered goes to its snapshot; entering brings it back; pin keeps it', () => {
    const p = page('p', 0, { snap })
    const pinned = page('q', 400, { snap, pin: true })
    let state: LodState = emptyLodState()
    const at = (now: number, entered: string | null = null) => {
      const r = assignLod([p, pinned], { zoom: 1, view: VIEW, budget: 4, entered, now, state })
      state = r.state
      return r.lod
    }
    expect(at(0)).toEqual({ p: 'live', q: 'live' })
    expect(at(29_999).p).toBe('live')
    expect(at(30_000)).toEqual({ p: 'snapshot', q: 'live' })
    expect(at(31_000).p).toBe('snapshot') // stays demoted
    expect(at(32_000, 'p').p).toBe('live')
    expect(at(40_000).p).toBe('live') // entered at 32 s: 30 s from there
    expect(at(62_000).p).toBe('snapshot')
  })

  it('a page with no snapshot is not demoted to a chip', () => {
    const p = page('p', 0)
    const r = assignLod([p], { zoom: 1, view: VIEW, budget: 4, entered: null, now: 60_000, state: { since: { p: 0 }, demoted: {} } })
    expect(r.lod.p).toBe('live')
  })
})
