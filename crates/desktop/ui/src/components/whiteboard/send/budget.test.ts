import { describe, expect, it } from 'vitest'
import { type BudgetImage, LONG_SIDE, MAX_IMAGES, MAX_PIXELS, fitBudget, mergeFrames } from './budget'

const img = (card: string | null, w: number, h: number, extra: Partial<BudgetImage> = {}): BudgetImage => ({
  card,
  marks: [`m-${card ?? 'free'}-${extra.zoomOf ?? ''}`],
  world: [0, 0, w, h],
  rect: null,
  px: [w, h],
  ...extra,
})
const total = (xs: { w: number; h: number }[]) => xs.reduce((n, i) => n + i.w * i.h, 0)

describe('fitBudget', () => {
  it('keeps a small send as it is, long side capped at 768', () => {
    const r = fitBudget([img('a', 1600, 800), img('b', 300, 200)])
    expect(r.omitted).toBe(0)
    expect(r.images.map((i) => [i.w, i.h])).toEqual([
      [768, 384],
      [300, 200],
    ])
  })

  it('scales everything down together before dropping anything', () => {
    // Three 768×768 images = 1.77 MP: scaling alone fits without going under the legibility floor.
    const r = fitBudget([img('a', 768, 768), img('b', 768, 768), img('c', 768, 768, { zoomOf: 'm-a-' })])
    expect(r.omitted).toBe(0)
    expect(r.images).toHaveLength(3)
    expect(total(r.images)).toBeLessThanOrEqual(MAX_PIXELS)
    const [a, b] = r.images
    expect(a?.w).toBe(b?.w)
    expect(a?.w).toBeLessThan(768)
  })

  it('then drops the zoom images', () => {
    const cards = Array.from({ length: 5 }, (_, i) => img(`c${i}`, 768, 768))
    const zooms = Array.from({ length: 3 }, (_, i) => img(`c${i}`, 768, 768, { zoomOf: `m-c${i}-` }))
    const r = fitBudget([...cards, ...zooms])
    expect(r.images.every((i) => !i.zoomOf)).toBe(true)
    expect(r.images).toHaveLength(5)
    expect(r.omitted).toBe(3)
    expect(total(r.images)).toBeLessThanOrEqual(MAX_PIXELS)
  })

  it('then merges the images of one frame', () => {
    const xs = Array.from({ length: 9 }, (_, i) => img(`c${i}`, 768, 768, { frame: i < 6 ? 'F' : undefined, world: [i * 800, 0, 768, 768] }))
    const r = fitBudget(xs)
    expect(r.images.length).toBeLessThanOrEqual(MAX_IMAGES)
    expect(r.images).toHaveLength(4)
    const merged = r.images[0]
    expect(merged?.card).toBeNull()
    expect(merged?.marks).toHaveLength(6)
    expect(r.omitted).toBe(0)
    expect(total(r.images)).toBeLessThanOrEqual(MAX_PIXELS)
  })

  it('never carries more than 6 images or 1.2 MP', () => {
    const xs = Array.from({ length: 14 }, (_, i) => img(`c${i}`, 2000, 1500))
    const r = fitBudget(xs)
    expect(r.images).toHaveLength(MAX_IMAGES)
    expect(r.omitted).toBe(8)
    expect(total(r.images)).toBeLessThanOrEqual(MAX_PIXELS)
    for (const i of r.images) expect(Math.max(i.w, i.h)).toBeLessThanOrEqual(LONG_SIDE)
  })
})

describe('mergeFrames', () => {
  it('unions the boxes and keeps order by first member', () => {
    const out = mergeFrames([img('a', 100, 100, { frame: 'F', world: [0, 0, 100, 100] }), img('x', 50, 50), img('b', 100, 100, { frame: 'F', world: [200, 50, 100, 100] })])
    expect(out).toHaveLength(2)
    expect(out[0]?.world).toEqual([0, 0, 300, 150])
    expect(out[0]?.px).toEqual([300, 150])
    expect(out[1]?.card).toBe('x')
  })
})
