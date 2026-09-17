// 43 §7: read a reply aloud. The harness synthesizes and streams one Opus packet per
// `SpeechAudio` event; this module decodes (libopus as wasm) and schedules each packet
// back-to-back on one `AudioContext`, gapless as long as packets arrive before `next`.

import { useQuery } from '@tanstack/react-query'
import { OpusDecoder, type OpusDecoderSampleRate } from 'opus-decoder'
import { useSyncExternalStore } from 'react'
import { toast } from 'sonner'
import { queryKeys, useApi } from '@/lib/rpc'
import { RpcError } from '@/rpc/client'
import type { FinishReason, ModelState } from '@/rpc/extra'
import { on } from '@/store/sessions'

let ctx: AudioContext | null = null
let dec: OpusDecoder<OpusDecoderSampleRate> | null = null
/** `ctx` time the next packet starts at. */
let next = 0
/** Utterance being played; packets for any other id are late and dropped. */
let id: string | null = null

/** Begin utterance `speech`: stop the previous one, open a context and a decoder at `sampleRate`. */
export function start(speech: string, sampleRate: number): void {
  stop()
  ctx = new AudioContext({ sampleRate })
  next = 0
  id = speech
  // The wire is 24 kHz (43 §0.5); the decoder accepts only libopus rates, anything else falls back to 48 kHz.
  dec = new OpusDecoder({ channels: 1, sampleRate: sampleRate as OpusDecoderSampleRate })
}

/** Decode one base64 Opus packet of `speech` and schedule it after the previous one; ignored for a retired id. */
export function push(speech: string, b64: string): void {
  const d = dec
  if (!d || speech !== id) return
  // `ready` is the wasm instantiation (ms; cached after the first utterance). `.then` on a settled
  // promise keeps packet order, and lets the first packets of an utterance queue behind it.
  void d.ready.then(() => {
    if (dec !== d || !ctx) return
    const { channelData } = d.decodeFrame(Uint8Array.from(atob(b64), (c) => c.charCodeAt(0)))
    const pcm = channelData[0]
    if (!pcm?.length) return
    const buf = ctx.createBuffer(1, pcm.length, ctx.sampleRate)
    // The decoder copies out of wasm memory into a plain `Float32Array`; the lib types leave the buffer kind open.
    buf.copyToChannel(pcm as Float32Array<ArrayBuffer>, 0)
    const src = ctx.createBufferSource()
    src.buffer = buf
    src.connect(ctx.destination)
    const at = Math.max(ctx.currentTime + 0.02, next)
    src.start(at)
    next = at + buf.duration
  })
}

/** Seconds of scheduled audio still to play. */
export function remaining(): number {
  return ctx ? Math.max(0, next - ctx.currentTime) : 0
}

/** Halt playback now and release the context and decoder. */
export function stop(): void {
  void ctx?.close()
  dec?.free()
  ctx = dec = null
  id = null
}

/** The utterance the UI shows as speaking: its `SpeechId` and the assistant entry it reads. */
export type Speaking = { id: string; entry: string } | null

let speaking: Speaking = null
const subs = new Set<() => void>()

/** Set (or clear) what the buttons show as speaking. */
export function setSpeaking(s: Speaking): void {
  speaking = s
  for (const f of subs) f()
}

const subscribe = (cb: () => void) => {
  subs.add(cb)
  return () => {
    subs.delete(cb)
  }
}

/** The utterance in flight, re-rendering on change. */
export function useSpeaking(): Speaking {
  return useSyncExternalStore(subscribe, () => speaking)
}

const MODEL_TOAST = 'voice-model'

function onModel(state: ModelState): void {
  if (state.state === 'loading') toast.loading('Loading voice model…', { id: MODEL_TOAST })
  else if (state.state === 'ready') toast.dismiss(MODEL_TOAST)
  else if (state.state === 'failed') toast.error(state.message, { id: MODEL_TOAST })
}

function onFinished(speech: string, reason: FinishReason): void {
  const clear = () => {
    if (speaking?.id === speech) setSpeaking(null)
    if (id === speech) stop()
  }
  if (reason.reason === 'done') {
    // Every packet is published but the tail is still scheduled; wait for the queued pushes, then for playback.
    void (dec?.ready ?? Promise.resolve()).then(() => setTimeout(clear, remaining() * 1000))
    return
  }
  if (reason.reason === 'error') toast.error(reason.message)
  clear()
}

/** Wire `VoiceEvent`s to the player and the toasts; once per app boot. Returns the unsubscribe. */
export function installSpeechListeners(): () => void {
  const offs = [
    on('VoiceModel', (e) => onModel(e.state)),
    on('SpeechStarted', (e) => start(e.id, e.sample_rate)),
    on('SpeechAudio', (e) => push(e.id, e.opus)),
    on('SpeechFinished', (e) => onFinished(e.id, e.reason)),
  ]
  return () => {
    for (const off of offs) off()
  }
}

/** Whether this harness reads aloud: `voice_status` answered. `-32001` (plugin disabled) is final; other failures retry. */
export function useVoice(): boolean {
  const api = useApi()
  return useQuery({
    queryKey: queryKeys.voice,
    queryFn: () => api.voice_status(),
    staleTime: Number.POSITIVE_INFINITY,
    retry: (n, e) => n < 3 && !(e instanceof RpcError && e.code === -32001),
  }).isSuccess
}
