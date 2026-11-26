import { describe, expect, it } from 'vitest'
import { SETTLE_MS, following, step, webId } from './web-live'

const clip = { x: 0, y: 0, width: 1000, height: 800 }
const at = (x: number, y = 100) => ({ x, y, width: 300, height: 200 })

describe('webId', () => {
  it('is a stable 16-hex id, distinct per key', () => {
    const a = webId('canvas:01J0000000000000000000000A')
    expect(a).toMatch(/^[0-9a-f]{16}$/)
    expect(webId('canvas:01J0000000000000000000000A')).toBe(a)
    expect(webId('sheet:01J0000000000000000000000A')).not.toBe(a)
  })
})

describe('step', () => {
  it('attaches once the rectangle held still, hides on a move, shows again after it settles', () => {
    const s = following()
    expect(step(s, at(10), clip, 0)).toBeNull()
    expect(step(s, at(10), clip, SETTLE_MS - 1)).toBeNull()
    expect(step(s, at(10), clip, SETTLE_MS)).toBe('attach')
    expect(step(s, at(10), clip, SETTLE_MS + 16)).toBeNull()
    expect(step(s, at(40), clip, 500)).toBe('hide')
    expect(step(s, at(70), clip, 516)).toBeNull()
    expect(step(s, at(70), clip, 516 + SETTLE_MS)).toBe('show')
  })

  it('stays hidden while the rectangle is partly outside the clip, and hides when it leaves', () => {
    const s = following()
    step(s, at(800), clip, 0)
    expect(step(s, at(800), clip, 1000)).toBeNull()
    step(s, at(10), clip, 1000)
    expect(step(s, at(10), clip, 2000)).toBe('attach')
    expect(step(s, at(10), null, 2016)).toBe('hide')
  })
})
