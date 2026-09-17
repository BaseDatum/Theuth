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
import { useConnection } from '@/store/connection'
import { on, useSessions } from '@/store/sessions'

let ctx: AudioContext | null = null
/** The module's one reusable context, made inside a user gesture (45 §7.3): iOS WKWebView will not start one made outside. */
let audioCtx: AudioContext | null = null
let dec: OpusDecoder<OpusDecoderSampleRate> | null = null
/** `ctx` time the next packet starts at. */
let next = 0
/** Utterance being played; packets for any other id are late and dropped. */
let id: string | null = null

/** Create (once) and resume the shared context; call from a click. The wire is 24 kHz (43 §0.5). */
export function unlock(): void {
  audioCtx ??= new AudioContext({ sampleRate: 24_000 })
  void audioCtx.resume()
}

/** Begin utterance `speech`: stop the previous one, take the shared context when its rate matches (else open one) and a decoder at `sampleRate`. */
export function start(speech: string, sampleRate: number): void {
  stop()
  ctx = audioCtx?.sampleRate === sampleRate ? audioCtx : new AudioContext({ sampleRate })
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

/** Halt playback now and release the decoder and the context (the shared one stays open). */
export function stop(): void {
  if (ctx !== audioCtx) void ctx?.close()
  dec?.free()
  ctx = dec = null
  id = null
}

/** The utterance the UI shows as speaking: its `SpeechId` and the assistant entry it reads. */
export type Speaking = { id: string; entry: string } | null

let speaking: Speaking = null
const subs = new Set<() => void>()
/** `idle()` waiters, released when `speaking` clears. */
let idlers: (() => void)[] = []

/** Set (or clear) what the buttons show as speaking. */
export function setSpeaking(s: Speaking): void {
  speaking = s
  for (const f of subs) f()
  if (s) return
  const waiting = idlers
  idlers = []
  for (const r of waiting) r()
}

/** Resolves once nothing is speaking: `onFinished` clears `speaking` after the scheduled tail has played. */
function idle(): Promise<void> {
  if (!speaking) return Promise.resolve()
  const { promise, resolve } = Promise.withResolvers<void>()
  idlers.push(resolve)
  return promise
}

let queue: Promise<void> = Promise.resolve()

/**
 * Speak `text` (a prose block of assistant `entry`) after whatever is speaking finishes (45 §7.3):
 * `Speak` replaces the current utterance, so blocks wait their turn instead of cutting each other off.
 */
// ponytail: a `SpeechFinished` that never arrives stalls the queue for the session's life; reload clears it.
export function enqueue(session: string, entry: string, text: string): void {
  queue = queue
    .then(idle)
    .then(async () => {
      const api = useConnection.getState().api
      if (api) setSpeaking({ id: await api.speak(session, text), entry })
    })
    .catch((e: unknown) => console.warn('speak block', e))
}

/** Sessions in voice mode (last input was a voice note) → id of the assistant entry last queued, so a block is spoken once. */
const voiceMode = new Map<string, string | null>()

/** Enter or leave voice mode for `session`; a voice send enters, a typed one leaves (45 §7.2). Not persisted. */
export function setVoiceMode(session: string, on: boolean): void {
  if (on) voiceMode.set(session, null)
  else voiceMode.delete(session)
}

/**
 * Block boundary (`ToolCallPending`, `TurnFinished`): the reducer has already closed the prose before it into
 * its own assistant entry, so the newest assistant entry since the user's line is the block; speak it once.
 */
function speakBlock(session: string): void {
  if (!voiceMode.has(session)) return
  const entries = useSessions.getState().sessions[session]?.entries ?? []
  for (let i = entries.length - 1; i >= 0; i--) {
    const e = entries[i]
    if (!e || e.kind === 'user') return
    if (e.kind !== 'assistant') continue
    if (voiceMode.get(session) === e.id) return
    voiceMode.set(session, e.id)
    if (e.text.trim()) enqueue(session, e.id, e.text)
    return
  }
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

/** Wire `VoiceEvent`s to the player and the toasts, and block boundaries to the speaking queue; once per app boot. Returns the unsubscribe. */
export function installSpeechListeners(): () => void {
  const offs = [
    on('VoiceModel', (e) => onModel(e.state)),
    on('SpeechStarted', (e) => start(e.id, e.sample_rate)),
    on('SpeechAudio', (e) => push(e.id, e.opus)),
    on('SpeechFinished', (e) => onFinished(e.id, e.reason)),
    on('ToolCallPending', (e) => speakBlock(e.session)),
    on('TurnFinished', (e) => speakBlock(e.session)),
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
