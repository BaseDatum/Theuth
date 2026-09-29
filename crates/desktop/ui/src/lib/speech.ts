// 43 §7: read a reply aloud. The engine synthesizes and streams one Opus packet per
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
let dec: OpusDecoder<OpusDecoderSampleRate> | null = null
/** Rate of the utterance's packets. */
let rate = SAMPLE_RATE
/** `ctx` time the next packet starts at. */
let next = 0
/** Utterance being played; packets for any other id are late and dropped. */
let id: string | null = null
/**
 * Sources scheduled and not yet ended. `cut` stops them: a source that is merely disconnected, or left
 * scheduled through `suspend`, plays on (iOS: from where it stopped) the next time the context resumes.
 */
const active = new Set<AudioBufferSourceNode>()
/** Utterance whose every packet is published (`SpeechFinished { done }`); it ends when `active` drains. */
let done: string | null = null

/** Who holds the audio session: read-aloud, a dictation's mic tap (54 §4.5), a voice-note recording (45 §7.2), a talk's mic tap (76 §5.3). */
export type Use = 'speak' | 'dictate' | 'record' | 'talk'
const uses = new Set<Use>()

/**
 * iOS WKWebView files bare Web Audio under `AmbientSound`, which the ringer switch mutes (WebKit#251532);
 * `playback` plays like media, `play-and-record` keeps the mic open. The type is a hard override, so it
 * follows the holders: a mic use wins while speaking, and none → `auto` (WebKit's default), which lets iOS
 * deactivate the session and drop the Now Playing island. No-op where the API is absent.
 */
function session(): void {
  const nav = navigator as Navigator & { audioSession?: { type: string } }
  if (!nav.audioSession) return
  nav.audioSession.type =
    uses.has('dictate') || uses.has('record') || uses.has('talk') ? 'play-and-record' : uses.has('speak') ? 'playback' : 'auto'
}

/**
 * Hold the session for `use`: set the type, create (once) and resume the context. Mic uses call it inside
 * the tap, before `getUserMedia`; the context comes back so dictation's tap shares the one device-rate
 * context instead of opening a second one (WKWebView). Pair with `release(use)`.
 */
export function unlock(use: Use): AudioContext {
  // Nothing held: whatever is still scheduled belongs to a finished utterance and must not resume with us.
  if (uses.size === 0) cut()
  uses.add(use)
  session()
  ctx ??= new AudioContext()
  void ctx.resume()
  return ctx
}

/**
 * Let go of `use`. With no holder left, suspend the context and reset the type: a running context (or a
 * `play-and-record` type) left behind keeps iOS's Now Playing ("localhost") up. Suspended, not closed —
 * `unlock` resumes it without a gesture. Idempotent; a use never held releases nothing but the idle state.
 */
export async function release(use: Use): Promise<void> {
  uses.delete(use)
  if (uses.size === 0) {
    cut()
    await ctx?.suspend()
  }
  session()
}

/** Stop every scheduled source and forget the utterance's schedule; `drain` never fires for them. */
function cut(): void {
  for (const src of active) {
    src.onended = null
    src.stop()
    src.disconnect()
  }
  active.clear()
  done = null
  next = 0
  sched.length = 0
  if (heardNow !== null) {
    heardNow = null
    onHeard?.(null, true)
  }
}

/**
 * Utterances scheduled, oldest first, with the sources of each still to end: the head is what the speaker plays
 * NOW. A talk's sentences arrive seconds ahead of playback (synthesis outruns speech), so the agent subtitle
 * follows this, not the engine's `Saying` (TALK-UI-A A1); `onHeard` is the talk store's callback.
 */
const sched: { speech: string; left: number }[] = []
let heardNow: string | null = null
/** `cut`: the `null` is a stop (`Flush`, a new read-aloud), not the tail running out. */
let onHeard: ((speech: string | null, cut?: boolean) => void) | null = null

/** Report a change of the utterance heard. An empty schedule with packets still to come (underrun) is not the end. */
function heard(): void {
  const head = sched[0]?.speech ?? null
  if (head === null && heardNow !== null && mine.has(heardNow)) return
  if (head === heardNow) return
  heardNow = head
  onHeard?.(head)
}

/** Unlock Web Audio on the click that starts speak (43 §2.3) and warm the decoder during the RPC round-trip. */
export function prime(sampleRate = SAMPLE_RATE): void {
  unlock('speak')
  if (!dec) dec = new OpusDecoder({ channels: 1, sampleRate: sampleRate as OpusDecoderSampleRate })
}

/** Begin utterance `speech`: cut previous audio, keep the unlocked context, open a decoder. */
export function start(speech: string, sampleRate: number): void {
  unlock('speak')
  cut()
  begin(speech, sampleRate)
}

/**
 * Begin utterance `speech` behind whatever is still scheduled (76 §0.4: a talk's sentences are queued, each its
 * own id, published back-to-back): no cut, `next` carries over, so the join is gapless.
 */
function startQueued(speech: string, sampleRate: number): void {
  unlock('speak')
  begin(speech, sampleRate)
}

function begin(speech: string, sampleRate: number): void {
  id = speech
  rate = sampleRate
  played = -1
  dec?.free()
  dec = new OpusDecoder({ channels: 1, sampleRate: sampleRate as OpusDecoderSampleRate })
}

/** Highest `seq` of the current utterance whose packet has finished playing; `-1` before the first. */
let played = -1

/** `talk_audio`'s `played_seq`: the heard prefix of the current utterance on this client (76 §4.6), `null` when none. */
export const playedSeq = (): number | null => (played < 0 ? null : played)

/** Decode one base64 Opus packet (`seq` within `speech`) and schedule it after the previous one; ignored for a retired id. */
export function push(speech: string, b64: string, seq = -1): void {
  const d = dec
  if (!d || speech !== id) return
  // `ready` is the wasm instantiation (ms; cached after the first utterance). `.then` on a settled
  // promise keeps packet order, and lets the first packets of an utterance queue behind it.
  void d.ready.then(() => {
    if (dec !== d || !ctx) return
    const { channelData } = d.decodeFrame(Uint8Array.from(atob(b64), (c) => c.charCodeAt(0)))
    const pcm = channelData[0]
    if (!pcm?.length) return
    const buf = ctx.createBuffer(1, pcm.length, rate)
    // The decoder copies out of wasm memory into a plain `Float32Array`; the lib types leave the buffer kind open.
    buf.copyToChannel(pcm as Float32Array<ArrayBuffer>, 0)
    const src = ctx.createBufferSource()
    src.buffer = buf
    src.connect(ctx.destination)
    const at = Math.max(ctx.currentTime + 0.02, next)
    src.start(at)
    next = at + buf.duration
    active.add(src)
    let tail = sched.at(-1)
    if (tail?.speech !== speech) sched.push((tail = { speech, left: 0 }))
    tail.left++
    // Nothing scheduled before it: it plays from `at`.
    if (sched.length === 1) heard()
    src.onended = () => {
      active.delete(src)
      if (id === speech && seq > played) played = seq
      const head = sched[0]
      if (head?.speech === speech && --head.left === 0) {
        sched.shift()
        heard()
      }
      drain()
    }
  })
}

/** The last scheduled source of a fully published utterance has played: clear it and give back `speak`. */
function drain(): void {
  if (done === null || active.size > 0) return
  const speech = done
  done = null
  clear(speech)
}

/** Utterance `speech` is over: drop the badge and, if it is the one playing (or none is), the `speak` hold. */
function clear(speech: string): void {
  if (speaking?.id === speech) setSpeaking(null)
  // `id === null`: finished (error) before its `SpeechStarted`; `stop` only drops `prime`'s hold then.
  if (id === speech || id === null) stop()
}

/** Halt playback now, release the decoder and the session's `speak` hold (see `release`). */
export function stop(): void {
  cut()
  void release('speak')
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

/** Resolves once nothing is speaking: `drain` clears `speaking` after the scheduled tail has played. */
function idle(): Promise<void> {
  if (!speaking) return Promise.resolve()
  const { promise, resolve } = Promise.withResolvers<void>()
  idlers.push(resolve)
  return promise
}

/**
 * Utterances this client asked for. The engine publishes every `Speech*` event to every subscriber
 * (desktop and phone alike); only the requester plays them, or a phone's tap comes out of the laptop.
 */
const mine = new Set<string>()
let inflight = 0
/** A `SpeechStarted` that arrived while a `speak` reply was in flight: held until the id is claimed. */
let held: { id: string; sampleRate: number; packets: string[] } | null = null
/**
 * Session whose speech this client plays for a talk it started with `sink: client` (76 §5.3); `null` otherwise.
 * Its `client`-sink utterances are claimed on `SpeechStarted` by session, ahead of any `Saying`, and queued gaplessly;
 * a `device` one on the same session (a talk on the daemon's speaker) is not this client's to play.
 */
let talkSession: string | null = null

/**
 * Enter (`session`) or leave (`null`) talk playback; leaving cuts and frees the decoder. `Flush` mid-talk is `stop()`.
 * `heard(speech | null, cut?)` reports the utterance the speaker plays now (see `sched`); `cut` marks a stop.
 */
export function setTalk(session: string | null, heard?: (speech: string | null, cut?: boolean) => void): void {
  talkSession = session
  onHeard = heard ?? null
  if (session === null) stop()
}

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
  } catch (e) {
    // `prime` took the `speak` hold on the tap; nothing will play, so give it back (unless another utterance is on).
    if (id === null) stop()
    throw e
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
  if (reason.reason === 'done') {
    // Every packet is published but the tail is still scheduled: wait for the queued pushes, then `drain` on the
    // last `onended` — not a timer off `currentTime`, which iOS lets drift from what has actually played.
    void (dec?.ready ?? Promise.resolve()).then(() => {
      done = speech
      drain()
    })
    return
  }
  if (reason.reason === 'error') toast.error(reason.message)
  clear(speech)
}

/** Wire `VoiceEvent`s to the player and the toasts, and block boundaries to the speaking queue; once per app boot. Returns the unsubscribe. */
export function installSpeechListeners(): () => void {
  const offs = [
    on('VoiceModel', (e) => onModel(e.state)),
    on('SpeechStarted', (e) => {
      if (mine.has(e.id)) start(e.id, e.sample_rate)
      else if (talkSession !== null && e.session === talkSession && e.sink === 'client') {
        mine.add(e.id)
        startQueued(e.id, e.sample_rate)
      } else if (inflight > 0) held = { id: e.id, sampleRate: e.sample_rate, packets: [] }
    }),
    on('SpeechAudio', (e) => {
      if (mine.has(e.id)) push(e.id, e.opus, e.seq)
      else if (held?.id === e.id) held.packets.push(e.opus)
    }),
    on('SpeechFinished', (e) => {
      if (held?.id === e.id) held = null
      if (mine.has(e.id) || speaking?.id === e.id) onFinished(e.id, e.reason)
    }),
    on('ToolCallPending', (e) => speakBlock(e.session)),
    on('TurnFinished', (e) => speakBlock(e.session)),
  ]
  // Belt and braces: backgrounded with nothing held, make sure the session is idle (a missed release would
  // otherwise keep the island up until the next tap).
  const hidden = () => {
    if (document.visibilityState === 'hidden' && uses.size === 0) void release('speak')
  }
  document.addEventListener('visibilitychange', hidden)
  return () => {
    for (const off of offs) off()
    document.removeEventListener('visibilitychange', hidden)
  }
}

/** Whether this engine reads aloud: `voice_status` answered. `-32001` (plugin disabled) is final; other failures retry. */
export function useVoice(): boolean {
  const api = useApi()
  return useQuery({
    queryKey: queryKeys.voice,
    queryFn: () => api.voice_status(),
    staleTime: Number.POSITIVE_INFINITY,
    retry: (n, e) => n < 3 && !(e instanceof RpcError && e.code === -32001),
  }).isSuccess
}
