import { describe, expect, it } from 'vitest'
import { CANVAS_BG, contrast, inkCss, INK_COLOURS, nearestInk, readable } from './colour'

describe('ink colours (§8.6, A1)', () => {
  it('theme flip: ink is near-black on paper and bone on soot; named colours use their dark twins', () => {
    expect(inkCss('ink', false)).toBe('#1a1612')
    expect(inkCss('ink', true)).toBe('#e8dcc8')
    for (const [name, [light, dark]] of Object.entries(INK_COLOURS)) {
      expect(inkCss(name as keyof typeof INK_COLOURS, false)).toBe(light)
      expect(inkCss(name as keyof typeof INK_COLOURS, true)).toBe(dark)
    }
  })

  it('every named colour keeps ≥ 3 : 1 against its canvas (A1)', () => {
    for (const [light, dark] of Object.values(INK_COLOURS)) {
      expect(contrast(light, CANVAS_BG.light)).toBeGreaterThanOrEqual(3)
      expect(contrast(dark, CANVAS_BG.dark)).toBeGreaterThanOrEqual(3)
    }
  })

  it("light green and amber are the contract's hues with lightness moved just far enough", () => {
    expect(readable('#2f9e44', false)).toBe(INK_COLOURS.green[0])
    expect(readable('#b7791f', false)).toBe(INK_COLOURS.amber[0])
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
    expect(nearestInk('#8f2f1f')).toBe('red')
    expect(nearestInk('#90301f')).toBe('red')
    expect(nearestInk('#00ffcc')).toBe('#00ffcc')
  })
})
