import { describe, expect, it } from 'vitest'
import * as Y from 'yjs'
import { mkBoard, mkCard, mkMark, mkStroke } from './fixtures/testing'
import { LOCAL, readBoard, type Box } from './model'
import { commitSend, planSend, type Caption } from './send'

const VIEW: Box = [0, 0, 2000, 1500]

describe('planSend (§8.6)', () => {
  const big = mkCard('big', 0, 0, 400, 800, { kind: 'page' })
  const small = mkCard('small', 600, 0, 100, 100)

  it('one image per inked card; a second, zoomed one when the mark is under 15 % of the card', () => {
    const b = mkBoard({
      cards: [big, small],
      ink: [
        mkStroke('tiny', [10, 10, 20, 20], { card: 'big', ver: 1, cs: [400, 800] }), // 400 px² ≪ 15 % of 320 000
        mkStroke('wide', [0, 200, 400, 400], { card: 'big', ver: 1, cs: [400, 800], at: 60_000 }), // 50 % of the card
        mkStroke('s', [10, 10, 60, 60], { card: 'small', ver: 1, cs: [100, 100] }), // 36 %: no zoom
      ],
    })
    const plan = planSend(b, { view: VIEW })
    expect(plan.marks).toHaveLength(3)
    const cardImages = plan.images.filter((i) => !i.zoomOf)
    expect(cardImages.map((i) => [i.card, i.marks.length, i.world])).toEqual(
      expect.arrayContaining([['big', 2, [0, 0, 400, 800]], ['small', 1, [600, 0, 100, 100]]]),
    )
    const zooms = plan.images.filter((i) => i.zoomOf)
    expect(zooms).toHaveLength(1)
    const tiny = plan.marks.find((m) => m.strokes[0] === 'tiny')!
    expect(zooms[0]).toMatchObject({ card: 'big', marks: [tiny.id], zoomOf: tiny.id })
    expect(zooms[0]!.world[2]).toBeGreaterThanOrEqual(160)
  })

  it('a free mark: its box padded 24 px, at least 160 × 160', () => {
    const b = mkBoard({ ink: [mkStroke('f', [1000, 1000, 300, 20])] })
    const plan = planSend(b, { view: VIEW })
    expect(plan.images).toEqual([{ card: null, marks: [plan.marks[0]!.id], rect: null, world: [976, 930, 348, 160] }])
    expect(plan.marks[0]!.card).toBeUndefined()
  })

  it('off-screen fresh ink is counted, not sent — unless asked', () => {
    const b = mkBoard({ ink: [mkStroke('in', [10, 10, 10, 10]), mkStroke('o1', [9000, 0, 10, 10]), mkStroke('o2', [9000, 5000, 10, 10])] })
    const p = planSend(b, { view: VIEW })
    expect(p.marks.map((m) => m.strokes)).toEqual([['in']])
    expect(p.offScreen).toBe(2)
    const all = planSend(b, { view: VIEW, offScreen: true })
    expect(all.marks).toHaveLength(3)
    expect(all.offScreen).toBe(0)
  })

  it('window rects come from the handle only for fully visible boxes', () => {
    const b = mkBoard({ cards: [small], ink: [mkStroke('s', [10, 10, 60, 60], { card: 'small', ver: 1, cs: [100, 100] })] })
    const rect = { x: 1, y: 2, width: 3, height: 4 } as DOMRect
    const p = planSend(b, { view: VIEW, windowRect: (w) => (w[0] === 600 ? rect : null) })
    expect(p.images[0]!.rect).toBe(rect)
  })

  it('captions: a page mark takes the describe-point answer nearest its centre; excerpts get lines', () => {
    const ex = mkCard('ex', 0, 1000, 400, 300, { kind: 'excerpt', versions: [{ source: { t: 'excerpt', path: 'a.ts', start: 10, end: 40, text: '' }, at: 0, by: 'owner' }] })
    const b = mkBoard({
      cards: [big, ex],
      ink: [
        mkStroke('p1', [100, 700, 10, 10], { card: 'big', ver: 1, cs: [400, 800] }),
        mkStroke('p2', [104, 712, 10, 10], { card: 'big', ver: 1, cs: [400, 800] }),
        mkStroke('e', [20, 30 + 8 + 18 * 2, 100, 30], { card: 'ex', ver: 1, cs: [400, 300] }),
      ],
    })
    const captions = new Map<string, Caption>([
      ['p1', { selector: 'nav > a:nth-child(3)', role: 'tab', label: 'tab bar, item 3', rect: [0, 0, 1, 1], at: [105, 705] }],
      ['p2', { selector: 'main', role: 'main', label: 'list', rect: [0, 0, 1, 1], at: [300, 100] }],
    ])
    const plan = planSend(b, { view: VIEW, captions })
    const page = plan.marks.find((m) => m.card === 'big')!
    expect(page.region).toEqual({ kind: 'page', label: 'tab bar, item 3', selector: 'nav > a:nth-child(3)', role: 'tab', rect: [0, 0, 1, 1] })
    const lines = plan.marks.find((m) => m.card === 'ex')!
    expect(lines.region).toEqual({ kind: 'lines', label: 'lines 12–13', lines: [12, 13] })
  })
})

describe('commitSend (§2.7)', () => {
  it('answers older sent marks, writes the new marks as sent, all wb-local', () => {
    const doc = new Y.Doc()
    doc.transact(() => {
      const old = new Y.Map<unknown>()
      for (const [k, v] of Object.entries({ strokes: ['x'], bb: [0, 0, 1, 1], state: 'sent', sent_at: 1, by: 'owner' })) old.set(k, v)
      doc.getMap('marks').set('OLD', old)
      const pin = new Y.Map<unknown>()
      for (const [k, v] of Object.entries({ strokes: ['y'], bb: [0, 0, 1, 1], state: 'pinned', sent_at: 1, by: 'owner' })) pin.set(k, v)
      doc.getMap('marks').set('PIN', pin)
    })
    const origins: unknown[] = []
    doc.on('update', (_: Uint8Array, o: unknown) => origins.push(o))
    const b = mkBoard({ ink: [mkStroke('s1', [0, 0, 10, 10])], marks: [mkMark('OLD', ['x']), mkMark('PIN', ['y'], { state: 'pinned' })] })
    const plan = planSend(b, { view: VIEW })
    commitSend(doc, b, plan, { to: 'sess', hw: { [plan.marks[0]!.id]: 'no third\ntab <b>' }, blobs: [plan.marks[0]!.id], now: 42 })
    expect(origins.every((o) => o === LOCAL)).toBe(true)
    const after = readBoard(doc)
    expect(after.marks.OLD!.state).toBe('answered')
    expect(after.marks.PIN!.state).toBe('pinned')
    const m = after.marks[plan.marks[0]!.id]!
    expect(m).toMatchObject({ strokes: ['s1'], state: 'sent', sent_at: 42, to: ['sess'], blob: true, hw: 'no third tab  b ' })
    expect(after.markOf.s1).toBe(m.id)
  })
})
