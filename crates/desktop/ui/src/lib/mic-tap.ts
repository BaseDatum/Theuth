// The browser-side microphone graph a relay-linked client streams to the daemon (54 §4.5, 76 §5.3):
// mic → `theuth-tap` worklet → 100 ms i16 batches → an RPC push. Shared by dictation and talk; the
// caller owns the current `Tap` and the audio-session `Use` it was opened under.

import { Batcher } from '@/lib/pcm'
import * as speech from '@/lib/speech'

export type Tap = { stream: MediaStream; ctx: AudioContext; nodes: AudioNode[]; batcher: Batcher | null }

/** Contexts whose worklet already has `theuth-tap`; a second `registerProcessor` of the same name rejects. */
const tapped = new WeakSet<AudioContext>()

/** One `AudioWorkletProcessor` that posts each 128-frame render quantum of its mono input to the main thread. */
const WORKLET_SRC = `registerProcessor('theuth-tap', class extends AudioWorkletProcessor {
  process(inputs) { const ch = inputs[0][0]; if (ch) this.port.postMessage(ch.slice()); return true }
})`

/**
 * Open the mic inside the click gesture (the permission prompt needs it) on speech's one device-rate
 * context; the daemon resamples (ponytail: ~1 Mbit/s of PCM at 48 kHz — decimate here if the relay minds).
 */
export async function openTap(use: speech.Use): Promise<Tap> {
  speech.unlock(use)
  const stream = await navigator.mediaDevices.getUserMedia({
    audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true },
  })
  // Esc during the prompt may have released the hold with nothing to tear down; take it again (a
  // gesture-free resume is fine once the context exists) or the tap renders nothing.
  const ctx = speech.unlock(use)
  const t: Tap = { stream, ctx, nodes: [], batcher: null }
  if (ctx.sampleRate % 50 !== 0) {
    closeTap(t)
    throw new Error('unsupported microphone rate')
  }
  return t
}

/**
 * Mic → tap → 100 ms i16 batches → `emit(b64)`; never awaits inside the audio path. `live()` is read after the
 * worklet module loads: `false` means the tap was closed meanwhile (Esc, a fast finish) and nothing is wired.
 */
export async function wireTap(t: Tap, emit: (b64: string) => void, live: () => boolean): Promise<void> {
  const { ctx } = t
  const batcher = new Batcher(ctx.sampleRate / 10, emit)
  // A tap has to sit on a path to `destination` or the graph never renders it; zero gain keeps it silent.
  const mute = ctx.createGain()
  mute.gain.value = 0
  mute.connect(ctx.destination)
  let tap: AudioNode
  if (ctx.audioWorklet) {
    if (!tapped.has(ctx)) {
      // `blob:` needs `script-src blob:` in the Tauri CSP (tauri.conf.json).
      const url = URL.createObjectURL(new Blob([WORKLET_SRC], { type: 'text/javascript' }))
      try {
        await ctx.audioWorklet.addModule(url)
      } finally {
        URL.revokeObjectURL(url)
      }
      tapped.add(ctx)
    }
    if (!live()) return
    const node = new AudioWorkletNode(ctx, 'theuth-tap')
    node.port.onmessage = (e: MessageEvent<Float32Array>) => batcher.push(e.data)
    tap = node
  } else {
    // ponytail: `createScriptProcessor` is deprecated but the only tap where `audioWorklet` is missing.
    const node = ctx.createScriptProcessor(2048, 1, 1)
    node.onaudioprocess = (e) => batcher.push(e.inputBuffer.getChannelData(0))
    tap = node
  }
  const src = ctx.createMediaStreamSource(t.stream)
  src.connect(tap).connect(mute)
  t.nodes = [src, tap, mute]
  t.batcher = batcher
}

/** Stop the mic tracks and disconnect the graph; the caller releases its audio-session hold. */
export function closeTap(t: Tap): void {
  for (const track of t.stream.getTracks()) track.stop()
  for (const n of t.nodes) n.disconnect()
}
