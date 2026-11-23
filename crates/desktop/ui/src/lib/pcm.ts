// 54 §4.5: the wire form of remote dictation audio. The daemon takes mono little-endian i16 PCM,
// base64 in a JSON string; the browser hands us Float32 frames per render quantum. This file is the
// pure part (encode + batch) so it can be tested without an `AudioContext`.
// ponytail: raw PCM at 16 kHz is ~32 kB/s per phone; Opus via WebCodecs `AudioEncoder` when WKWebView (iOS 26+) is the floor.

import { encode } from './base64'

/** Float32 frames in [-1, 1] → base64 of little-endian i16, clamped and rounded. */
export function toI16Base64(frames: Float32Array): string {
  const bytes = new Uint8Array(frames.length * 2)
  const view = new DataView(bytes.buffer)
  for (let i = 0; i < frames.length; i++) {
    const f = frames[i] ?? 0
    view.setInt16(i * 2, Math.round(Math.max(-1, Math.min(1, f)) * 32767), true)
  }
  return encode(bytes)
}

/** Accumulates frames and emits whole `samples`-long batches as base64 i16; `flush` emits the remainder. */
export class Batcher {
  private buf: Float32Array
  private len = 0

  constructor(
    private samples: number,
    private emit: (b64: string) => void,
  ) {
    this.buf = new Float32Array(samples)
  }

  /** Append `frames`, emitting every full batch they complete. */
  push(frames: Float32Array): void {
    let at = 0
    while (at < frames.length) {
      const n = Math.min(this.samples - this.len, frames.length - at)
      this.buf.set(frames.subarray(at, at + n), this.len)
      this.len += n
      at += n
      if (this.len === this.samples) {
        this.emit(toI16Base64(this.buf))
        this.len = 0
      }
    }
  }

  /** Emit whatever is buffered (possibly nothing). */
  flush(): void {
    if (this.len === 0) return
    this.emit(toI16Base64(this.buf.subarray(0, this.len)))
    this.len = 0
  }
}
