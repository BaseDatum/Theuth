// 43 §7: read a reply aloud. The harness synthesizes and streams one Opus packet per
// `SpeechAudio` event; this module decodes (libopus as wasm) and schedules each packet
// back-to-back on one `AudioContext`, gapless as long as packets arrive before `next`.

import { useQuery } from '@tanstack/react-query'
import { OpusDecoder, type OpusDecoderSampleRate } from 'opus-decoder'
import { useSyncExternalStore } from 'react'
import { toast } from 'sonner'
import { queryKeys, useApi } from '@/lib/rpc'
import type { Api } from '@/rpc/api'
import { RpcError } from '@/rpc/client'
import type { FinishReason, ModelState } from '@/rpc/extra'
import { useConnection } from '@/store/connection'
import { on, useSessions } from '@/store/sessions'

/** PCM rate `ptts` produces and the packets decode to (43 §0.5). */
const SAMPLE_RATE = 24_000

/**
 * The module's one context, at the hardware rate, made and resumed inside a user gesture and never closed:
 * iOS WKWebView will not start one made outside a gesture, and one built at a forced `sampleRate` ran
 * silent there while reporting `running`. Packets go in as 24 kHz buffers and the graph resamples.
 */
let ctx: AudioContext | null = null
/** Master tap so a new utterance can mute in-flight BufferSources without closing `ctx`. */
let out: GainNode | null = null
let dec: OpusDecoder<OpusDecoderSampleRate> | null = null
/** Rate of the utterance's packets. */
let rate = SAMPLE_RATE
/** `ctx` time the next packet starts at. */
let next = 0
/** Utterance being played; packets for any other id are late and dropped. */
let id: string | null = null

/**
 * iOS WKWebView files bare Web Audio under `AmbientSound`, which the ringer switch mutes (WebKit#251532);
 * `playback` plays like media, `play-and-record` keeps the mic open. The type is a hard override, so each
 * side sets its own before touching audio. No-op where the API is absent.
 */
function session(type: 'playback' | 'play-and-record'): void {
  const nav = navigator as Navigator & { audioSession?: { type: string } }
  if (nav.audioSession) nav.audioSession.type = type
}

function graph(): AudioContext {
  if (!ctx) {
    ctx = new AudioContext()
    out = ctx.createGain()
    out.connect(ctx.destination)
  }
  void ctx.resume()
  return ctx
}

/** Create (once) and resume the context; call from the record tap, before `getUserMedia`. Returns it so
 * dictation's mic tap shares the one device-rate context instead of opening a second one (WKWebView). */
export function unlock(): AudioContext {
  session('play-and-record')
  return graph()
}

function cut(): void {
  if (!ctx || !out) {
    next = 0
    return
  }
  out.disconnect()
  out = ctx.createGain()
  out.connect(ctx.destination)
  next = 0
}

/** Unlock Web Audio on the click that starts speak (43 §2.3) and warm the decoder during the RPC round-trip. */
export function prime(sampleRate = SAMPLE_RATE): void {
  session('playback')
  graph()
  if (!dec) dec = new OpusDecoder({ channels: 1, sampleRate: sampleRate as OpusDecoderSampleRate })
}

/** Begin utterance `speech`: cut previous audio, keep the unlocked context, open a decoder. */
// ponytail: `start` during an in-progress recording flips the mic to `playback`; a record button while speaking is not a flow yet.
export function start(speech: string, sampleRate: number): void {
  session('playback')
  graph()
  cut()
  id = speech
  rate = sampleRate
  dec?.free()
  dec = new OpusDecoder({ channels: 1, sampleRate: sampleRate as OpusDecoderSampleRate })
}

/** Decode one base64 Opus packet of `speech` and schedule it after the previous one; ignored for a retired id. */
export function push(speech: string, b64: string): void {
  const d = dec
  if (!d || speech !== id) return
  // `ready` is the wasm instantiation (ms; cached after the first utterance). `.then` on a settled
  // promise keeps packet order, and lets the first packets of an utterance queue behind it.
  void d.ready.then(() => {
    if (dec !== d || !ctx || !out) return
    const { channelData } = d.decodeFrame(Uint8Array.from(atob(b64), (c) => c.charCodeAt(0)))
    const pcm = channelData[0]
    if (!pcm?.length) return
    const buf = ctx.createBuffer(1, pcm.length, rate)
    // The decoder copies out of wasm memory into a plain `Float32Array`; the lib types leave the buffer kind open.
    buf.copyToChannel(pcm as Float32Array<ArrayBuffer>, 0)
    const src = ctx.createBufferSource()
    src.buffer = buf
    src.connect(out)
    const at = Math.max(ctx.currentTime + 0.02, next)
    src.start(at)
    next = at + buf.duration
  })
}

/** Seconds of scheduled audio still to play. */
export function remaining(): number {
  return ctx ? Math.max(0, next - ctx.currentTime) : 0
}

/**
 * Halt playback now and release the decoder. The context is suspended, not closed: a `playback`-type
 * context left running keeps iOS's Now Playing ("localhost") up; `start` resumes it without a gesture.
 */
export function stop(): void {
  cut()
  void ctx?.suspend()
  dec?.free()
  dec = null
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

/**
 * Utterances this client asked for. The harness publishes every `Speech*` event to every subscriber
 * (desktop and phone alike); only the requester plays them, or a phone's tap comes out of the laptop.
 */
const mine = new Set<string>()
let inflight = 0
/** A `SpeechStarted` that arrived while a `speak` reply was in flight: held until the id is claimed. */
let held: { id: string; sampleRate: number; packets: string[] } | null = null

/** `speak` over RPC, claiming the returned id for playback here; `entry` is the assistant entry it reads. */
export async function speak(api: Api, session: string, entry: string, text: string): Promise<void> {
  inflight++
  try {
    const sid = await api.speak(session, text)
    mine.add(sid)
    setSpeaking({ id: sid, entry })
    if (held?.id === sid) {
      start(sid, held.sampleRate)
      for (const p of held.packets) push(sid, p)
    }
    held = null
  } finally {
    inflight--
  }
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
      if (api) await speak(api, session, entry, text)
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
  mine.delete(speech)
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
    on('SpeechStarted', (e) => {
      if (mine.has(e.id)) start(e.id, e.sample_rate)
      else if (inflight > 0) held = { id: e.id, sampleRate: e.sample_rate, packets: [] }
    }),
    on('SpeechAudio', (e) => {
      if (mine.has(e.id)) push(e.id, e.opus)
      else if (held?.id === e.id) held.packets.push(e.opus)
    }),
    on('SpeechFinished', (e) => {
      if (held?.id === e.id) held = null
      if (mine.has(e.id) || speaking?.id === e.id) onFinished(e.id, e.reason)
    }),
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
