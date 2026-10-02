import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { VisualFrame } from '@/lib/audio/types'
import { analyserSource, levelSource, tapOrLevels } from '@/lib/visual-source'

const snap = (f: VisualFrame) => ({ bands: Array.from(f.bands), history: Array.from(f.history), start: f.historyStart, len: f.historyLength, peakDb: f.peakDb })

describe('levelSource', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())

  it('silence is flat bands at the peakDb floor', () => {
    const l = levelSource()
    const got: ReturnType<typeof snap>[] = []
    l.source.subscribe((f) => got.push(snap(f)))
    l.push([0])
    expect(got).toHaveLength(1)
    expect(got[0]!.bands.every((b) => b === 0)).toBe(true)
    expect(got[0]!.peakDb).toBeCloseTo(-80)
    l.idle()
    expect(got).toHaveLength(2)
    expect(got[1]!.bands.every((b) => b === 0)).toBe(true)
  })

  it('full level peaks in the low-mids and tapers to the floor at the top', () => {
    const l = levelSource()
    let f: ReturnType<typeof snap> | undefined
    l.source.subscribe((x) => (f = snap(x)))
    l.push([1])
    const peak = f!.bands.indexOf(Math.max(...f!.bands))
    expect(peak).toBeGreaterThanOrEqual(7)
    expect(peak).toBeLessThanOrEqual(11)
    expect(f!.bands[peak]!).toBeGreaterThan(0.85)
    expect(f!.bands[31]!).toBeLessThan(0.2)
    expect(f!.bands[0]!).toBeLessThan(f!.bands[peak]!)
    expect(f!.peakDb).toBe(0)
  })

  it('releases one frame per level, 20 ms apart, and wraps the 64-deep history ring', () => {
    const l = levelSource()
    const got: ReturnType<typeof snap>[] = []
    l.source.subscribe((f) => got.push(snap(f)))
    l.push([0.1, 0.2, 0.3])
    expect(got).toHaveLength(1)
    vi.advanceTimersByTime(20)
    expect(got).toHaveLength(2)
    vi.advanceTimersByTime(20)
    expect(got).toHaveLength(3)
    expect(got[2]!.history.slice(0, 3)).toEqual([0.1, 0.2, 0.3].map(Math.fround))
    expect(got[2]!.len).toBe(3)
    // 70 levels in total: the ring is full, the oldest six fell off the front.
    l.push(Array.from({ length: 67 }, (_, i) => (i + 4) / 100))
    vi.advanceTimersByTime(20 * 70)
    expect(got).toHaveLength(70)
    const last = got[69]!
    expect(last.len).toBe(64)
    expect(last.start).toBe(6)
    expect(last.history[last.start]).toBeCloseTo(0.07, 5)
    expect(last.history[(last.start + 63) % 64]).toBeCloseTo(0.7, 5)
  })

  it('dispose drops the queue and later pushes', () => {
    const l = levelSource()
    const got: number[] = []
    l.source.subscribe((f) => got.push(f.peakDb))
    l.push([0.5, 0.5, 0.5])
    l.dispose()
    vi.advanceTimersByTime(100)
    l.push([1])
    expect(got).toHaveLength(1)
  })
})

/** The Web Audio surface `createAnalyserTap` touches, with spies on the connections. */
function fakeContext() {
  const analyser = { fftSize: 0, smoothingTimeConstant: 0, frequencyBinCount: 512, connect: vi.fn(), disconnect: vi.fn() }
  const node = { connect: vi.fn(), disconnect: vi.fn() }
  const ctx = { sampleRate: 48_000, createAnalyser: vi.fn(() => analyser), createMediaStreamSource: vi.fn(() => node) }
  return { ctx: ctx as unknown as AudioContext, node, analyser }
}

describe('analyserSource', () => {
  it('taps a MediaStreamSource on the given context and disconnects it on dispose', () => {
    const { ctx, node, analyser } = fakeContext()
    const stream = {} as MediaStream
    const a = analyserSource(ctx, stream)
    expect(ctx.createMediaStreamSource).toHaveBeenCalledWith(stream)
    expect(node.connect).toHaveBeenCalledWith(analyser)
    expect(analyser.fftSize).toBe(1024)
    a.dispose()
    expect(node.disconnect).toHaveBeenCalledWith(analyser)
    expect(node.disconnect).toHaveBeenCalledWith()
  })
})

describe('tapOrLevels', () => {
  it('prefers the tap, else subscribes to levels and unsubscribes on dispose', () => {
    const { ctx } = fakeContext()
    const onLevels = vi.fn(() => vi.fn())
    const withTap = tapOrLevels({ ctx, stream: {} as MediaStream }, onLevels)
    expect(onLevels).not.toHaveBeenCalled()
    withTap.dispose()
    const off = vi.fn()
    const sub = vi.fn(() => off)
    const noTap = tapOrLevels(null, sub)
    expect(sub).toHaveBeenCalledTimes(1)
    noTap.dispose()
    expect(off).toHaveBeenCalledTimes(1)
  })
})
