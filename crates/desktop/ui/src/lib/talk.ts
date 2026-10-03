// 76 §5: the live spoken conversation as the desktop sees it. The engine (`crates/talk`) owns the loop; this
// module keeps the one talk's state and subtitles from `TalkEvent`s, starts/stops it over RPC, and — for a
// relay-linked client (§5.3) — streams the mic in and plays the Opus leg out through `speech.ts`.

import { useSyncExternalStore } from 'react'
import { toast } from 'sonner'
import { chime } from '@/lib/cue'
import { closeTap, openTap, type Tap, wireTap } from '@/lib/mic-tap'
import * as speech from '@/lib/speech'
import { useTalkSettings } from '@/lib/talk-settings'
import { LOCAL, useEngines } from '@/link/engines'
import type { Api } from '@/rpc/api'
import type { Aec } from '@/rpc/bindings/Aec'
import type { State } from '@/rpc/bindings/State'
import type { TalkEvent } from '@/rpc/bindings/TalkEvent'
import type { VoiceEvent } from '@/rpc/bindings/VoiceEvent'
import { on } from '@/store/sessions'

/** The orb's three looks (§0.12). */
export type OrbState = 'idle' | 'thinking' | 'speaking'

/** `TalkState` → orb: `Holding` keeps the speaking look (paused, not stopped); `Interrupted` drops to idle. */
export const orbState = (s: State): OrbState =>
  s === 'thinking' ? 'thinking' : s === 'speaking' || s === 'holding' ? 'speaking' : 'idle'

/**
 * TALK-UI-A A2: the muted line above the orb, mirroring the talk state. `Thinking…` = the turn started, no speech
 * yet; `Working…` = a tool call is running (the session's `activity`, since the talk state does not distinguish it)
 * while the model thinks, or between spoken sentences; `Holding` (a paused barge-in) keeps `Speaking` — nothing new.
 */
export function phaseLine(s: State, tool: boolean, saying: boolean): string {
  switch (s) {
    case 'listening':
      return 'Listening'
    case 'thinking':
      return tool ? 'Working…' : 'Thinking…'
    case 'speaking':
      return tool && !saying ? 'Working…' : 'Speaking'
    case 'holding':
      return 'Speaking'
    default:
      return ''
  }
}

/** TALK-UI-A A4: the muted hint under the orb — what a tap does now (`idle` also = no talk on this session). */
export function orbHint(s: State): string {
  return s === 'idle' ? 'Tap the orb to speak' : s === 'listening' || s === 'interrupted' ? 'Tap the orb to stop listening' : 'Tap the orb to stop'
}

/**
 * The two subtitle lines. `speech`: the utterance the agent line shows. `said`: every `Saying` of the talk by
 * utterance, for a client that plays the audio itself. `plays`: this client reported playback (`Playing`), so the
 * agent line follows what its speaker plays, not the engine's synthesis. `cut`: the sentence on the line was
 * stopped mid-way (a `Flush` or a non-`done` `SpeechFinished`), so the line waits for `Interrupted` or the next
 * `TalkState` instead of blanking. `frozen`: `Interrupted` — the line holds the sentence that was cut until the talk
 * leaves `listening` (the next turn) or ends.
 */
export type Subtitles = {
  agent: string
  speech: string | null
  user: string
  utts: string[]
  said: Record<string, string>
  plays: boolean
  cut: boolean
  frozen: boolean
}

export const NO_SUBTITLES: Subtitles = { agent: '', speech: null, user: '', utts: [], said: {}, plays: false, cut: false, frozen: false }

/** The string 53 shows and `Degraded { mic_no_aec }` means. */
export const NO_AEC = 'No echo cancellation; headphones recommended'

/**
 * `speech.ts` reports the utterance its speaker plays now; `null` = nothing (76 §5.3, client sink). `cut`: the
 * `null` is a `Flush`/stop, not the tail running out.
 */
export type Playing = { type: 'Playing'; speech: string | null; cut?: boolean }

export type SubEvent = TalkEvent | Extract<VoiceEvent, { type: 'SpeechFinished' }> | Playing

/**
 * Subtitle reducer (§4 of the brief; TALK-UI-A A1). Agent: the sentence being PLAYED, never streamed text that has
 * not been spoken. Engine speaker (device sink, ≤ 200 ms of ring ahead): `Saying` text until the matching
 * `SpeechFinished` or `Interrupted`. This client's speaker (client sink, synthesis seconds ahead): once a `Playing`
 * arrived, `Saying` only records the text and the line follows `Playing` (`null` clears); `SpeechFinished` —
 * synthesis done, not playback — is ignored. A cut (barge-in): the line freezes on the sentence that was cut and
 * clears with the state — when the talk leaves `listening` for the next turn, or ends.
 * User: `Heard` partials per utterance, append-only (a shorter partial never replaces a longer one, so the line
 * never flickers), the final replaces, `Submitted` clears the turn.
 */
export function applySubtitles(s: Subtitles, e: SubEvent): Subtitles {
  switch (e.type) {
    case 'Saying': {
      const said = { ...s.said, [e.speech]: e.text }
      if (s.frozen) return { ...s, said }
      if (!s.plays) return { ...s, said, agent: e.text, speech: e.speech, cut: false }
      // The text for the sentence already playing (its first packet beat the `Saying`).
      return s.speech === e.speech ? { ...s, said, agent: e.text } : { ...s, said }
    }
    case 'Playing':
      if (s.frozen) return { ...s, plays: true }
      if (e.speech === null) return e.cut ? { ...s, plays: true, speech: null, cut: true } : { ...s, plays: true, agent: '', speech: null, cut: false }
      return { ...s, plays: true, agent: s.said[e.speech] ?? '', speech: e.speech, cut: false }
    case 'SpeechFinished':
      if (s.frozen || s.plays || s.speech !== e.id) return s
      return e.reason.reason === 'done' ? { ...s, agent: '', speech: null } : { ...s, speech: null, cut: true }
    case 'Interrupted':
      return { ...s, speech: null, said: {}, cut: false, frozen: true }
    case 'TalkState':
      // The frozen line goes when the next turn starts; a cut that no `Interrupted` claimed (an Esc elsewhere) goes now.
      if (s.frozen) return e.state === 'listening' || e.state === 'holding' || e.state === 'interrupted' ? s : { ...s, agent: '', speech: null, frozen: false }
      return s.cut ? { ...s, agent: '', speech: null, cut: false } : s
    case 'Heard': {
      const utts = s.utts.slice()
      while (utts.length <= e.utterance) utts.push('')
      const cur = utts[e.utterance] ?? ''
      if (e.final || e.text.length >= cur.length || !cur.startsWith(e.text)) utts[e.utterance] = e.text
      return { ...s, utts, user: utts.filter(Boolean).join(' ') }
    }
    case 'Submitted':
      return { ...s, user: '', utts: [] }
    default:
      return s
  }
}

/** The one talk per daemon as this client knows it; `mine` = this client started it and owns its audio leg. */
export type Talk = {
  phase: 'off' | 'starting' | 'on' | 'stopping'
  id: string | null
  session: string | null
  state: State
  aec: Aec | null
  /** The quiet caption under the subtitles (`Degraded`), else `null`. */
  degraded: string | null
  subs: Subtitles
  mine: boolean
}

export const OFF: Talk = { phase: 'off', id: null, session: null, state: 'idle', aec: null, degraded: null, subs: NO_SUBTITLES, mine: false }

let talk: Talk = OFF
const subs = new Set<() => void>()
/** The relay client's mic graph while a `client`-sink talk of ours runs. */
let tap: Tap | null = null
/** 82 §D: raw `TalkLevel` packets of the talk, for the waveform when there is no `tap` (local engine). */
const levelSubs = new Set<(levels: number[]) => void>()

/** The mic graph of the running talk, if this browser captures it (relay client); else `null`. */
export const currentTap = (): Tap | null => tap

/** Each level packet (eight 20 ms RMS values, oldest first) of the talk. Returns the unsubscribe. */
export function onLevels(cb: (levels: number[]) => void): () => void {
  levelSubs.add(cb)
  return () => {
    levelSubs.delete(cb)
  }
}
/** A2: `Submitted` seen and the turn has not left `listening` yet — the next `thinking` is the model taking it. */
let committed = false

function set(next: Talk): void {
  if (next === talk) return
  talk = next
  for (const cb of subs) cb()
}

const snapshot = () => talk
const subscribe = (cb: () => void) => {
  subs.add(cb)
  return () => {
    subs.delete(cb)
  }
}

/** The store, re-rendering on change. */
export function useTalk(): Talk {
  return useSyncExternalStore(subscribe, snapshot, snapshot)
}

function endAudio(): void {
  if (tap) {
    closeTap(tap)
    tap = null
  }
  void speech.release('talk')
  speech.setTalk(null)
}

/** Reducer over the events; exported for the simulator, which feeds the same path the bus does. */
export function handle(e: SubEvent): void {
  if (e.type === 'SpeechFinished' || e.type === 'Playing') {
    set({ ...talk, subs: applySubtitles(talk.subs, e) })
    return
  }
  // Not ours: another client started one, or `TalkFinished` for a talk we never saw.
  if (talk.id !== null && e.id !== talk.id && e.type !== 'TalkStarted') return
  switch (e.type) {
    case 'TalkStarted':
      committed = false
      set({ ...talk, phase: 'on', id: e.id, session: e.session, aec: e.aec, state: 'listening', subs: NO_SUBTITLES })
      return
    case 'TalkState':
      // A2: the turn cue — the state leaves `listening` for `thinking` after the user's utterance was committed
      // (`Submitted`). Not a typed turn elsewhere (no `Submitted`), not a barge-in resume (`Holding` → `Speaking`),
      // never while the user speaks (the gate closed before the submit). Only the client that owns the talk chimes.
      if (e.state === 'thinking' && talk.state === 'listening' && committed && talk.mine && useTalkSettings.getState().cue) {
        chime(speech.unlock('talk'))
      }
      if (e.state !== 'listening') committed = false
      set({ ...talk, state: e.state, subs: applySubtitles(talk.subs, e) })
      return
    case 'TalkLevel':
      for (const cb of levelSubs) cb(e.rms)
      return
    case 'Degraded':
      set({ ...talk, degraded: NO_AEC })
      return
    case 'Flush':
      if (talk.mine) speech.stop()
      return
    case 'Submitted':
      // The user's utterance is committed; `TalkState thinking` follows and plays the cue.
      committed = true
      set({ ...talk, subs: applySubtitles(talk.subs, e) })
      return
    case 'TalkFinished':
      committed = false
      endAudio()
      set(OFF)
      if (e.reason.reason === 'error') toast.error(e.reason.message)
      return
    default:
      set({ ...talk, subs: applySubtitles(talk.subs, e) })
  }
}

/** Wire `TalkEvent`s (and `SpeechFinished` for the agent line) to the store; once per app boot. Returns the unsubscribe. */
export function installTalkListeners(): () => void {
  const offs = [
    on('TalkStarted', handle),
    on('TalkState', handle),
    on('Heard', handle),
    on('TalkLevel', handle),
    on('Saying', handle),
    on('Submitted', handle),
    on('Interrupted', handle),
    on('Flush', handle),
    on('Degraded', handle),
    on('TalkFinished', handle),
    on('SpeechFinished', handle),
  ]
  return () => {
    for (const off of offs) off()
  }
}

/** `talk_status` → attach to a talk another window (or this one, before a reload) is running; ours no longer. */
export async function refreshStatus(api: Api): Promise<void> {
  const st = await api.talk_status()
  if (!st.active) {
    if (!talk.mine) set(OFF)
    return
  }
  if (talk.id === st.active.id) {
    set({ ...talk, phase: 'on', state: st.active.state, aec: st.active.aec })
    return
  }
  set({ ...OFF, phase: 'on', id: st.active.id, session: st.active.session, state: st.active.state, aec: st.active.aec })
}

/**
 * Start a talk on `session` (§5.1). A local peer hands the daemon the devices (`sink: device`); a relay client
 * (`useEngines.active !== LOCAL`, the dictation rule) taps its own mic inside this gesture and plays the Opus
 * leg (`sink: client`). Already talking on `session` → nothing to do; on another → refused by the daemon
 * ("already talking") and toasted.
 */
export async function start(api: Api, session: string): Promise<void> {
  if (talk.phase !== 'off') return
  set({ ...OFF, phase: 'starting', session, mine: true })
  const remote = useEngines.getState().active !== LOCAL
  // Inside the gesture: the context the A2 chime plays on (a relay client's tap takes the same hold again).
  speech.unlock('talk')
  let id: string | null = null
  try {
    const t = remote ? await openTap('talk') : null
    tap = t
    if (t) speech.setTalk(session, (playing, cut) => handle({ type: 'Playing', speech: playing, cut: cut === true }))
    const tid = await api.talk_start(session, t ? 'client' : 'device', t ? t.ctx.sampleRate : null)
    id = tid
    // `talk` was narrowed above and TS keeps that across awaits; read it fresh. `TalkStarted` may have beaten the reply.
    if (snapshot().phase === 'starting') set({ ...snapshot(), phase: 'on', id: tid })
    if (t) await wireTap(t, (b64) => void api.talk_audio(tid, b64, speech.playedSeq()).catch(() => undefined), () => tap === t)
  } catch (e) {
    endAudio()
    if (id !== null) void api.talk_stop(id).catch(() => undefined)
    set(OFF)
    toast.error(e instanceof Error ? e.message : String(e))
    void refreshStatus(api).catch(() => undefined)
  }
}

/**
 * The sidebar's Talk and the composer's Talk button: `session ?? new_session()`, then start (inside the user's
 * gesture — a relay client's mic needs it). Returns the session to navigate to; a talk already running on another
 * session is left alone (the daemon holds one) and that session comes back instead.
 */
export async function open(api: Api, session: string | null): Promise<string> {
  if (talk.phase !== 'off' && talk.session) return talk.session
  const target = session ?? (await api.new_session())
  void start(api, target)
  return target
}

/** Stop the running talk (the orb's click, Ctrl/Cmd+T); `TalkFinished` completes the transition. */
export async function stop(api: Api): Promise<void> {
  if (talk.id === null || talk.phase === 'stopping') return
  set({ ...talk, phase: 'stopping' })
  tap?.batcher?.flush()
  await api.talk_stop(talk.id).catch((e: unknown) => {
    endAudio()
    set(OFF)
    toast.error(e instanceof Error ? e.message : String(e))
  })
}
