import { describe, expect, it, vi } from 'vitest'
import type { NativeStroke, StrokeBatch } from '@/lib/ink-native'
import type { CapturedStroke } from './canvas/api'
import { createNativeBridge, toCaptured } from './native-ink'

const stroke = (seq: number, over: Partial<NativeStroke> = {}): NativeStroke => ({
  seq,
  tool: 'pen',
  colour: '#d9480f',
  width: 4,
  t0: 1_000 + seq,
  view: { tx: 100, ty: 50, zoom: 2 },
  pts: [100, 50, 0.5, 0, 120, 70, 1, 8],
  retry: 0,
  ...over,
})
const batch = (n: number, strokes: NativeStroke[]): StrokeBatch => ({ batch: n, strokes })

describe('native ink bridge', () => {
  it('converts to world with the stroke’s own view', () => {
    const c = toCaptured(stroke(1))
    expect(c.pts).toEqual([
      [0, 0, 0.5, 0],
      [10, 10, 1, 8],
    ])
    expect(c.width).toBe(2)
    expect(c.colour).toBe('red')
    expect(c.hasPressure).toBe(true)
    expect(c.at).toBe(1_001)
    // A different view on another stroke of the same batch converts with that view.
    const other = toCaptured(stroke(2, { view: { tx: 0, ty: 0, zoom: 1 } }))
    expect(other.pts[1]).toEqual([120, 70, 1, 8])
  })

  it('commits in seq order, acks the whole batch after the commit', async () => {
    const order: string[] = []
    const committed: CapturedStroke[][] = []
    const b = createNativeBridge({
      commit: async (s) => {
        order.push('commit')
        committed.push(s)
        return s.map((_, i) => `id${i}`)
      },
      ack: (seqs) => order.push(`ack:${seqs.join(',')}`),
    })
    await b.onBatch(batch(1, [stroke(2), stroke(1)]))
    expect(order).toEqual(['commit', 'ack:1,2'])
    expect(committed[0]?.map((s) => s.at)).toEqual([1_001, 1_002])
  })

  it('de-duplicates by seq: a resend is acked but not committed again', async () => {
    const commit = vi.fn(async (s: CapturedStroke[]) => s.map(() => 'x'))
    const acks: number[][] = []
    const b = createNativeBridge({ commit, ack: (s) => acks.push(s) })
    await b.onBatch(batch(1, [stroke(1), stroke(2)]))
    await b.onBatch(batch(2, [stroke(2, { retry: 1 }), stroke(3)]))
    await b.onBatch(batch(3, [stroke(3, { retry: 1 })]))
    expect(commit).toHaveBeenCalledTimes(2)
    expect(commit.mock.calls[1]?.[0]).toHaveLength(1)
    expect(acks).toEqual([[1, 2], [2, 3], [3]])
  })

  it('a resend arriving while the first delivery commits waits and is skipped', async () => {
    const { promise, resolve } = Promise.withResolvers<string[]>()
    const commit = vi.fn(() => promise)
    const acks: number[][] = []
    const b = createNativeBridge({ commit, ack: (s) => acks.push(s) })
    const first = b.onBatch(batch(1, [stroke(1)]))
    const again = b.onBatch(batch(2, [stroke(1, { retry: 1 })]))
    resolve(['a'])
    await Promise.all([first, again])
    expect(commit).toHaveBeenCalledTimes(1)
    expect(acks).toEqual([[1], [1]])
  })

  it('a failed commit acks nothing (the plugin resends)', async () => {
    const acks: number[][] = []
    const b = createNativeBridge({ commit: async () => Promise.reject(new Error('no canvas')), ack: (s) => acks.push(s) })
    await b.onBatch(batch(1, [stroke(1)]))
    expect(acks).toEqual([])
    const ok = createNativeBridge({ commit: async (s) => s.map(() => 'x'), ack: (s) => acks.push(s) })
    await ok.onBatch(batch(1, [stroke(1)]))
    expect(acks).toEqual([[1]])
  })
})
