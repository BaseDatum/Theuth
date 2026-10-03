// 82 §A: the two `VisualFrame` sources behind `VoiceWave`. A site with a `MediaStream` in this UI (a relay client's
// dictation/talk `Tap`, the voice-note recorder) taps it with an analyser; a site whose engine captures natively
// (the local engine) only gets 20 ms level packets over RPC and synthesises frames from them.

import { createFrameEmitter } from '@/lib/audio/frame-source'
import type { FrameSource, VisualFrame } from '@/lib/audio/types'
import { createAnalyserTap } from '@/hooks/use-audio-analyser'

export type VisualSource = { source: FrameSource<VisualFrame>; dispose: () => void }

/**
 * Analyser on `stream` inside `ctx`. For a `Tap` pass `tap.ctx` + `tap.stream`: a second `MediaStreamSource` on the
 * same stream is fine; the worklet path is untouched. `dispose` drops the analyser and the source node only — the
 * caller owns the stream and the context.
 */
export function analyserSource(ctx: AudioContext, stream: MediaStream): VisualSource {
  const node = ctx.createMediaStreamSource(stream)
  const tap = createAnalyserTap(ctx, node, { bands: BANDS, fftSize: 1024, smoothing: 0.5 })
  return {
    source: tap.visual,
    dispose: () => {
      tap.dispose()
      node.disconnect()
    },
  }
}

const BANDS = 32
const HISTORY = 64
/** Spacing of the engine's level samples; frames are released at this pace, not per packet. */
const STEP_MS = 20
/** `20·log10(1e-4)`: the floor of `peakDb` for a silent level. */
const FLOOR = 1e-4

/** Fixed spectral shape: a raised cosine over `i/31` centred on the low-mids (0.3) with a 0.15 floor. */
const ENVELOPE = Float32Array.from({ length: BANDS }, (_, i) => {
  const w = Math.min(1, Math.abs(i / (BANDS - 1) - 0.3) / 0.7)
  return 0.15 + 0.85 * 0.5 * (1 + Math.cos(Math.PI * w))
})

export type LevelSource = VisualSource & {
  /** Queue one packet (oldest first); each level becomes a frame 20 ms after the previous one. */
  push: (levels: readonly number[]) => void
  /** Drop the queue and show silence. */
  idle: () => void
}

/**
 * Levels (0..1, 20 ms each) → frames. Bands are `L · ENVELOPE` with a small wobble keyed on the sample count, so the
 * line never freezes into one bump; `history` is the last 64 levels; no `timeDomain` (`wave` mode never reads it).
 */
export function levelSource(): LevelSource {
  const emitter = createFrameEmitter<VisualFrame>()
  const frame: VisualFrame = { bands: new Float32Array(BANDS), history: new Float32Array(HISTORY), historyStart: 0, historyLength: 0, peakDb: 20 * Math.log10(FLOOR) }
  const queue: number[] = []
  let timer: number | NodeJS.Timeout | undefined
  let seq = 0
  let disposed = false

  const emit = (level: number) => {
    const l = Math.min(1, Math.max(0, level))
    seq++
    for (let i = 0; i < BANDS; i++) frame.bands[i] = Math.min(1, l * ENVELOPE[i]! * (1 + 0.1 * Math.sin(i * 0.9 + seq * 0.6)))
    if (frame.historyLength < HISTORY) {
      frame.history[(frame.historyStart + frame.historyLength) % HISTORY] = l
      frame.historyLength++
    } else {
      frame.history[frame.historyStart] = l
      frame.historyStart = (frame.historyStart + 1) % HISTORY
    }
    frame.peakDb = 20 * Math.log10(Math.max(l, FLOOR))
    emitter.emit(frame)
  }
  const drain = () => {
    timer = undefined
    const next = queue.shift()
    if (next === undefined || disposed) return
    emit(next)
    if (queue.length) timer = setTimeout(drain, STEP_MS)
  }
  const clear = () => {
    queue.length = 0
    clearTimeout(timer)
    timer = undefined
  }

  return {
    source: emitter,
    push: (levels) => {
      if (disposed) return
      queue.push(...levels)
      if (timer === undefined) drain()
    },
    idle: () => {
      clear()
      if (!disposed) emit(0)
    },
    dispose: () => {
      disposed = true
      clear()
    },
  }
}

/** A site's source: the analyser when the UI holds the stream, else levels from `onLevels` (an unsubscribe-returning subscription). */
export function tapOrLevels(tap: { ctx: AudioContext; stream: MediaStream } | null, onLevels: (cb: (levels: number[]) => void) => () => void): VisualSource {
  if (tap) return analyserSource(tap.ctx, tap.stream)
  const l = levelSource()
  const off = onLevels(l.push)
  return {
    source: l.source,
    dispose: () => {
      off()
      l.dispose()
    },
  }
}
