import { describe, expect, it } from 'vitest'
import { CANVAS_BG, contrast, inkCss, INK_COLOURS, nearestInk } from './colour'

describe('ink colours (§8.6, A1)', () => {
  it('theme flip: ink is near-black on paper and bone on soot; named colours use their dark twins', () => {
    expect(inkCss('ink', false)).toBe('#1a1612')
    expect(inkCss('ink', true)).toBe('#e8dcc8')
    for (const [name, [light, dark]] of Object.entries(INK_COLOURS)) {
      expect(inkCss(name as keyof typeof INK_COLOURS, false)).toBe(light)
      expect(inkCss(name as keyof typeof INK_COLOURS, true)).toBe(dark)
    }
  })

  it('named colours: ≥ 3 : 1 on soot; on bone the contract palette keeps ≥ 2.5 (green 2.54 and amber 2.69 are the contract’s values)', () => {
    for (const [light, dark] of Object.values(INK_COLOURS)) {
      expect(contrast(light, CANVAS_BG.light)).toBeGreaterThanOrEqual(2.5)
      expect(contrast(dark, CANVAS_BG.dark)).toBeGreaterThanOrEqual(3)
    }
    for (const name of ['ink', 'red', 'blue', 'violet'] as const) expect(contrast(INK_COLOURS[name][0], CANVAS_BG.light)).toBeGreaterThanOrEqual(3)
  })

  it('a free #rrggbb is moved to ≥ 3 : 1 in both themes, keeping a readable colour', () => {
    for (const hex of ['#ffff00', '#f0e0d0', '#202020', '#000080', '#7f7f7f', '#00ffcc'] as const) {
      expect(contrast(inkCss(hex, false), CANVAS_BG.light)).toBeGreaterThanOrEqual(3)
      expect(contrast(inkCss(hex, true), CANVAS_BG.dark)).toBeGreaterThanOrEqual(3)
    }
    // Already readable: unchanged.
    expect(inkCss('#000080', false)).toBe('#000080')
  })

  it('a native picker hex snaps to the nearest token only when visibly the same', () => {
    expect(nearestInk('#d9480f')).toBe('red')
    expect(nearestInk('#da4910')).toBe('red')
    expect(nearestInk('#00ffcc')).toBe('#00ffcc')
  })
})
