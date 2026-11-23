// 156 D8: play one transcript segment through Web Audio. The window CSP has no say over `decodeAudioData`,
// unlike a `blob:` URL on an `<audio>` element, which WebKit refuses with `NotSupportedError` under Tauri's CSP.
// The engine cuts the WAV (`meeting_segment_audio`); this file only pads the range, decodes the bytes and keeps
// the one-at-a-time play/stop state. Pure parts are exported for tests; the player takes its context lazily.

import { decode } from './base64'

/** Padding on each side of a segment, so clipped consonants at the boundaries are heard. */
export const PAD_MS = 150

/** `[t0 - pad, t1 + pad]` clamped at zero. */
export const padRange = (t0: number, t1: number, pad = PAD_MS): [number, number] => [Math.max(0, t0 - pad), t1 + pad]

/** The slice of `AudioContext` the player uses, so tests can stub it. */
export type PlayerContext = Pick<AudioContext, 'resume' | 'decodeAudioData' | 'createBufferSource' | 'destination'>

/** Fetches the base64 WAV for a padded `[t0, t1]`. */
export type FetchWav = (t0: number, t1: number) => Promise<string>

/**
 * One segment at a time. `toggle(id, …)` on the playing (or loading) row stops it; on another row it stops the
 * previous one first. `onChange` reports the row whose marker should show (set at click time, cleared on stop,
 * end or failure). Clicks that overtake a pending fetch drop its result.
 */
export class SegmentPlayer {
  private source: AudioBufferSourceNode | null = null
  private current: number | null = null
  private seq = 0

  constructor(
    private readonly ctx: () => PlayerContext | null,
    private readonly onChange: (playing: number | null) => void,
  ) {}

  get playing(): number | null {
    return this.current
  }

  stop(): void {
    this.seq++
    const src = this.source
    this.source = null
    if (src) {
      src.onended = null
      try {
        src.stop()
      } catch {
        // Already ended.
      }
    }
    if (this.current !== null) {
      this.current = null
      this.onChange(null)
    }
  }

  /** Rejects with the fetch or decode error; the marker is already cleared by then. */
  async toggle(id: number, t0: number, t1: number, fetch: FetchWav): Promise<void> {
    if (this.current === id) {
      this.stop()
      return
    }
    this.stop()
    const ctx = this.ctx()
    if (!ctx) throw new Error('audio is not available in this window')
    const seq = this.seq
    this.current = id
    this.onChange(id)
    // Must run synchronously inside the user gesture, before any await.
    void ctx.resume()
    try {
      const [p0, p1] = padRange(t0, t1)
      const b64 = await fetch(p0, p1)
      if (seq !== this.seq) return
      const buffer = await ctx.decodeAudioData(decode(b64).buffer)
      if (seq !== this.seq) return
      const src = ctx.createBufferSource()
      src.buffer = buffer
      src.connect(ctx.destination)
      src.onended = () => {
        if (this.source === src) {
          this.source = null
          this.current = null
          this.onChange(null)
        }
      }
      this.source = src
      src.start()
    } catch (e) {
      if (seq === this.seq) {
        this.current = null
        this.onChange(null)
      }
      throw e
    }
  }
}

/** Plain words for the toast; never a bare DOMException name. */
export const playError = (e: unknown): string => `Could not play this segment: ${e instanceof Error ? e.message : String(e)}`
