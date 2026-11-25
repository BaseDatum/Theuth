import { describe, expect, it } from 'vitest'
import { edgeMarks } from './edges'

const view = { left: 100, top: 50, width: 1000, height: 600 }
const at = (left: number, top: number) => ({ left, top, width: 100, height: 100 })

describe('edgeMarks', () => {
  it('nothing for a card in view, even partly', () => {
    expect(edgeMarks(view, [{ id: 'a', rect: at(500, 300) }, { id: 'b', rect: at(1050, 600) }])).toEqual([])
  })

  it('a card to the right gets a chevron on the right edge, pointing right, at its height', () => {
    const [m] = edgeMarks(view, [{ id: 'r', rect: at(3000, 300) }])
    expect(m?.x).toBeCloseTo(1000 - 28)
    expect(m?.y).toBeGreaterThan(250)
    expect(m?.y).toBeLessThan(400)
    expect(Math.abs(m?.angle ?? 99)).toBeLessThan(10)
  })

  it('a card above-left lands on the border of the inset box, view-relative', () => {
    const [m] = edgeMarks(view, [{ id: 'ul', rect: at(-5000, -5000) }])
    expect(m).toBeDefined()
    const onLeft = Math.abs((m?.x ?? 0) - 28) < 0.01
    const onTop = Math.abs((m?.y ?? 0) - 28) < 0.01
    expect(onLeft || onTop).toBe(true)
    expect(m?.angle).toBeLessThan(-90)
  })
})
