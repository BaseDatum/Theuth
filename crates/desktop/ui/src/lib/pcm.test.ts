import { expect, test } from 'vitest'
import { Batcher, toI16Base64 } from './pcm'

const decode = (b64: string): number[] => {
  const s = atob(b64)
  const out: number[] = []
  for (let i = 0; i < s.length; i += 2) {
    const u = s.charCodeAt(i) | (s.charCodeAt(i + 1) << 8)
    out.push(u >= 0x8000 ? u - 0x10000 : u)
  }
  return out
}

test('toI16Base64 is little-endian i16, clamped and rounded', () => {
  expect(decode(toI16Base64(new Float32Array([0, 0.25, -0.25, 1, -1, 2, -2, 1 / 32767])))).toEqual([
    0, 8192, -8192, 32767, -32767, 32767, -32767, 1,
  ])
  expect(toI16Base64(new Float32Array(0))).toBe('')
  // Past the 8 KiB chunk boundary the bytes still line up.
  const big = new Float32Array(10_000).fill(0.25)
  const got = decode(toI16Base64(big))
  expect(got.length).toBe(10_000)
  expect(got.every((v) => v === 8192)).toBe(true)
})

test('Batcher emits at the batch boundary and the remainder on flush', () => {
  const out: number[][] = []
  const b = new Batcher(4, (b64) => out.push(decode(b64)))
  const f = (...v: number[]) => new Float32Array(v.map((x) => x / 32767))
  b.push(f(1, 2, 3))
  expect(out).toEqual([])
  // Crosses one boundary and leaves a tail.
  b.push(f(4, 5, 6, 7, 8, 9))
  expect(out).toEqual([[1, 2, 3, 4], [5, 6, 7, 8]])
  b.flush()
  expect(out).toEqual([[1, 2, 3, 4], [5, 6, 7, 8], [9]])
  // Empty flush emits nothing.
  b.flush()
  expect(out.length).toBe(3)
})
