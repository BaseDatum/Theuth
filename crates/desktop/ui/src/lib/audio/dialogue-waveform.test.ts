import { describe, expect, it } from 'vitest'
import { type AudioFeatures, SILENT_FEATURES, WAVEFORM_CONSTANTS, WaveformEngine, visualFrameToAudioFeatures } from '@/lib/audio/dialogue-waveform'
import type { VisualFrame } from '@/lib/audio/types'

const DT = 1 / 60
const MAX_D = 20
const POINTS = 72
const SPEECH: AudioFeatures = { level: 0.7, low: 0.5, mid: 0.8, high: 0.3, transient: 0.4, brightness: 0.4 }

const run = (e: WaveformEngine, seconds: number, features: Readonly<AudioFeatures>, recording: boolean) => {
  for (let t = 0; t < seconds; t += DT) e.update(features, recording, DT)
}
const peak = (d: Float32Array) => Math.max(...Array.from(d, Math.abs))

const frame = (bands: number[], peakDb: number): VisualFrame => ({ bands: Float32Array.from(bands), history: new Float32Array(0), historyStart: 0, historyLength: 0, peakDb })

describe('WaveformEngine', () => {
  it('idles as a gentle ocean pinned at both ends', () => {
    const e = new WaveformEngine()
    run(e, 2, SPEECH, false)
    expect(e.packets).toHaveLength(0)
    const d = e.displacements(POINTS, MAX_D)
    expect(d[0]).toBeCloseTo(0, 5)
    expect(d[POINTS - 1]).toBeCloseTo(0, 5)
    expect(peak(d)).toBeGreaterThan(0.5)
    expect(peak(d)).toBeLessThanOrEqual(MAX_D * WAVEFORM_CONSTANTS.idleAmplitudeScale * 1.2)
  })

  it('emits packets at the right edge that travel left while recording speech', () => {
    const e = new WaveformEngine()
    e.update(SPEECH, true, DT)
    expect(e.packets).toHaveLength(1)
    const first = e.packets[0]!
    expect(first.pos).toBeLessThan(WAVEFORM_CONSTANTS.voicePacketSpawnPos)
    const before = first.pos
    run(e, 0.5, SPEECH, true)
    expect(first.pos).toBeLessThan(before)
    expect(e.packets.length).toBeGreaterThan(1)
    // Newer packets sit further right: spawn order is right → left travel order.
    for (let i = 1; i < e.packets.length; i++) expect(e.packets[i]!.pos).toBeGreaterThan(e.packets[i - 1]!.pos)
  })

  it('speech swings the line well past the idle ocean but never past the soft limit', () => {
    const idle = new WaveformEngine()
    const talk = new WaveformEngine()
    run(idle, 1.5, SILENT_FEATURES, false)
    run(talk, 1.5, SPEECH, true)
    const d = talk.displacements(POINTS, MAX_D)
    expect(peak(d)).toBeGreaterThan(peak(idle.displacements(POINTS, MAX_D)) * 1.5)
    expect(peak(d)).toBeLessThanOrEqual(MAX_D * 0.92)
  })

  it('builds pressure at the left sink once packets arrive there', () => {
    const e = new WaveformEngine()
    run(e, 1, SPEECH, true)
    expect(e.leftPressure).toBe(0)
    run(e, 3, SPEECH, true)
    expect(e.leftPressure).toBeGreaterThan(0.05)
  })

  it('settles back to the ocean when the recording falls silent, and drops everything when it stops', () => {
    const e = new WaveformEngine()
    run(e, 1, SPEECH, true)
    expect(e.packets.length).toBeGreaterThan(0)
    run(e, 6, SILENT_FEATURES, true)
    expect(e.packets).toHaveLength(0)
    expect(e.speechDrive).toBeLessThan(0.01)

    run(e, 1, SPEECH, true)
    e.update(SPEECH, false, DT)
    expect(e.packets).toHaveLength(0)
    expect(e.speechDrive).toBe(0)
    expect(e.leftPressure).toBe(0)
  })

  it('reset returns to the constructed state', () => {
    const e = new WaveformEngine()
    run(e, 2, SPEECH, true)
    e.reset()
    const fresh = new WaveformEngine()
    expect(e.packets).toHaveLength(0)
    expect(e.render).toEqual(fresh.render)
    expect(e.timeOffset).toBe(0)
    expect(e.lastPacketAt).toBe(-1)
    expect(Array.from(e.displacements(POINTS, MAX_D))).toEqual(Array.from(fresh.displacements(POINTS, MAX_D)))
  })

  it('caps each step at 50 ms and writes into a caller buffer', () => {
    const e = new WaveformEngine()
    e.update(SILENT_FEATURES, false, 5)
    expect(e.timeOffset).toBeCloseTo(0.05)
    const out = new Float32Array(POINTS)
    expect(e.displacements(POINTS, MAX_D, out)).toBe(out)
    expect(Array.from(e.displacements(POINTS, 0))).toEqual(new Array(POINTS).fill(0))
  })
})

describe('visualFrameToAudioFeatures', () => {
  it('silence is all zero, including a −∞ peak', () => {
    const { features, nextLevel } = visualFrameToAudioFeatures(frame(new Array(32).fill(0), Number.NEGATIVE_INFINITY), 0)
    expect(features).toEqual({ ...SILENT_FEATURES, brightness: 0 })
    expect(nextLevel).toBe(0)
  })

  it('a full frame saturates and a rise reads as a transient', () => {
    const { features, nextLevel } = visualFrameToAudioFeatures(frame(new Array(32).fill(1), 0), 0)
    expect(features.level).toBe(1)
    expect(features.low).toBe(1)
    expect(features.mid).toBe(1)
    expect(features.high).toBe(1)
    expect(features.transient).toBe(1)
    expect(nextLevel).toBeCloseTo(0.7)
    expect(visualFrameToAudioFeatures(frame(new Array(32).fill(1), 0), 1).features.transient).toBe(0)
  })

  it('splits the bands low → high and brightness follows the top', () => {
    const lowOnly = visualFrameToAudioFeatures(frame(Array.from({ length: 32 }, (_, i) => (i < 8 ? 0.5 : 0)), -12), 0).features
    expect(lowOnly.low).toBeGreaterThan(0)
    expect(lowOnly.mid).toBe(0)
    expect(lowOnly.high).toBe(0)
    expect(lowOnly.brightness).toBe(0)
    const highOnly = visualFrameToAudioFeatures(frame(Array.from({ length: 32 }, (_, i) => (i >= 20 ? 0.5 : 0)), -12), 0).features
    expect(highOnly.low).toBe(0)
    expect(highOnly.high).toBeGreaterThan(0)
    expect(highOnly.brightness).toBeGreaterThan(0.9)
    for (const v of Object.values(highOnly)) {
      expect(v).toBeGreaterThanOrEqual(0)
      expect(v).toBeLessThanOrEqual(1)
    }
  })
})
