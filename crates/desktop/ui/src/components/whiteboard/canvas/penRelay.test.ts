import { describe, expect, it } from 'vitest'
import { PEN, PEN_MAX_PTS, PEN_RELAY_JS, readPen } from './penRelay'

const VP = { w: 390, h: 844 }
const msg = (params: unknown) => ({ jsonrpc: '2.0', method: PEN, params })

describe('pen relay from an entered page', () => {
  it('accepts well-formed points in the frame and clamps pressure', () => {
    expect(readPen(msg({ phase: 'move', pts: [[10, 20, 1.4, 5], [11, 21, -1, 9]] }), VP)).toEqual({
      phase: 'move', pts: [[10, 20, 1, 5], [11, 21, 0, 9]],
    })
  })

  it('refuses anything a hostile page could use to write ink elsewhere or in bulk', () => {
    expect(readPen(msg({ phase: 'down', pts: [[5000, 20, 0.5, 0]] }), VP)).toBeNull() // far outside the frame
    expect(readPen(msg({ phase: 'down', pts: [[1, NaN, 0.5, 0]] }), VP)).toBeNull()
    expect(readPen(msg({ phase: 'scribble', pts: [[1, 1, 0.5, 0]] }), VP)).toBeNull()
    expect(readPen(msg({ phase: 'move', pts: Array.from({ length: PEN_MAX_PTS + 1 }, () => [1, 1, 0.5, 0]) }), VP)).toBeNull()
    expect(readPen({ jsonrpc: '2.0', method: 'ui/open-link', params: {} }, VP)).toBeNull()
  })

  it('the page-side script parses', () => {
    expect(() => new Function(PEN_RELAY_JS)).not.toThrow()
  })
})
