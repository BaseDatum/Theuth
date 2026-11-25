import { describe, expect, it } from 'vitest'
import { EXAMPLE_PTS_HEX } from '../canvas/fixtures/example'
import { MAX_POINTS, pack, rdp, split, unpack, type Pt } from './pack'

const hex = (b: Uint8Array) => [...b].map((x) => x.toString(16).padStart(2, '0')).join('')

describe('pack (§2.6)', () => {
  it('golden vector', () => {
    const o: [number, number] = [100, 50]
    const bytes = pack(o, [[100, 50, 0.5, 0], [101.5, 50.25, 1.0, 8], [99.5, 49, 0.2, 20]])
    expect(hex(bytes)).toBe(EXAMPLE_PTS_HEX)
    expect(bytes.length).toBe(18)
    expect(unpack(o, bytes)).toEqual([
      [100, 50, 128 / 255, 0],
      [101.5, 50.25, 1, 8],
      [99.5, 49, 51 / 255, 20],
    ])
  })

  it('round trip within ⅛ px, pressure within 1/255, time within the 4 ms grid', () => {
    const pts: Pt[] = Array.from({ length: 300 }, (_, i) => [37.3 + Math.sin(i / 9) * 140, -12.6 + i * 1.37, (i % 50) / 50, i * 7])
    const o: [number, number] = [pts[0]![0], pts[0]![1]]
    const back = unpack(o, pack(o, pts))
    expect(back).toHaveLength(pts.length)
    back.forEach(([x, y, p, t], i) => {
      const [ex, ey, ep, et] = pts[i]!
      expect(Math.abs(x - ex)).toBeLessThanOrEqual(1 / 16 + 1e-9)
      expect(Math.abs(y - ey)).toBeLessThanOrEqual(1 / 16 + 1e-9)
      expect(Math.abs(p - ep)).toBeLessThanOrEqual(1 / 255)
      expect(Math.abs(t - et)).toBeLessThanOrEqual(2 * i + 2) // per-step rounding may accumulate
    })
  })

  it('rounds half away from zero', () => {
    const o: [number, number] = [0, 0]
    const back = unpack(o, pack(o, [[0, 0, 0.5, 0], [-0.0625, 0.0625, 0.5, 4]]))
    expect(back[1]![0]).toBe(-0.125)
    expect(back[1]![1]).toBe(0.125)
  })

  it('long step is interpolated', () => {
    const o: [number, number] = [0, 0]
    const bytes = pack(o, [[0, 0, 0.5, 0], [10000, -5000, 1, 40]])
    const view = new DataView(bytes.buffer)
    for (let i = 0; i < bytes.length / 6; i++) {
      expect(Math.abs(view.getInt16(i * 6, true))).toBeLessThanOrEqual(32767)
      expect(Math.abs(view.getInt16(i * 6 + 2, true))).toBeLessThanOrEqual(32767)
    }
    const back = unpack(o, bytes)
    expect(back.length).toBe(4) // 80 000 eighths → three steps
    expect(back[back.length - 1]!.slice(0, 2)).toEqual([10000, -5000])
  })

  it('4 000-point split: each part ≤ 4 000 points and starts on the previous last point', () => {
    const pts: Pt[] = Array.from({ length: 9500 }, (_, i) => [i, Math.sin(i), 0.5, i * 4])
    const parts = split(pts)
    expect(parts.length).toBe(3)
    for (const p of parts) expect(p.length).toBeLessThanOrEqual(MAX_POINTS)
    expect(parts[1]![0]!.slice(0, 2)).toEqual(parts[0]![parts[0]!.length - 1]!.slice(0, 2))
    expect(parts[1]![0]![3]).toBe(0)
    expect(parts.reduce((n, p) => n + p.length, 0)).toBe(9500 + parts.length - 1)
    // A packed part stays under the 48 KiB base64 update cap with room to spare.
    const o: [number, number] = [parts[0]![0]![0], parts[0]![0]![1]]
    expect(pack(o, parts[0]!).length).toBe(24_000)
  })

  it('rdp keeps the ends and drops collinear points', () => {
    const line: Pt[] = Array.from({ length: 100 }, (_, i) => [i, 2 * i, 0.5, i])
    expect(rdp(line, 0.35)).toEqual([line[0], line[99]])
    const corner: Pt[] = [[0, 0, 0.5, 0], [5, 0, 0.5, 1], [10, 0, 0.5, 2], [10, 5, 0.5, 3], [10, 10, 0.5, 4]]
    expect(rdp(corner, 0.35).map((p) => p.slice(0, 2))).toEqual([[0, 0], [10, 0], [10, 10]])
  })
})
