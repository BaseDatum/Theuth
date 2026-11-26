import { describe, expect, it } from 'vitest'
import { dragIntent, menuOpens } from './input'

describe('pan, move or menu (§8.4)', () => {
  it("an unselected card's body pans the board; its title bar moves it", () => {
    expect(dragIntent('body', { mode: 'idle' })).toBe('pan')
    expect(dragIntent('header', { mode: 'idle' })).toBe('move')
  })

  it('a selected card moves by its body too', () => {
    expect(dragIntent('body', { mode: 'idle', selected: true })).toBe('move')
  })

  it('Space, the middle button and Hand pan everywhere, a selected card included', () => {
    for (const from of ['pane', 'header', 'body'] as const) {
      expect(dragIntent(from, { mode: 'idle', selected: true, space: true })).toBe('pan')
      expect(dragIntent(from, { mode: 'select', selected: true, button: 1 })).toBe('pan')
      expect(dragIntent(from, { mode: 'hand', selected: true })).toBe('pan')
    }
  })

  it('on the canvas: a plain drag pans, Shift-drag marquees; the select and frame tools keep their drag', () => {
    expect(dragIntent('pane', { mode: 'idle' })).toBe('pan')
    expect(dragIntent('pane', { mode: 'link' })).toBe('pan')
    expect(dragIntent('pane', { mode: 'idle', shift: true })).toBe('marquee')
    expect(dragIntent('pane', { mode: 'select' })).toBe('lasso')
    expect(dragIntent('pane', { mode: 'frame' })).toBe('frame')
  })

  it('a held press opens its menu only if the pointer stayed within 4 px', () => {
    expect(menuOpens(0, 0)).toBe(true)
    expect(menuOpens(2, 3)).toBe(true)
    expect(menuOpens(3, 3)).toBe(false)
    expect(menuOpens(4, 0)).toBe(false)
  })
})
