// 76 §5: the live spoken conversation as the desktop sees it. The engine (`crates/talk`) owns the loop; this
// module keeps the one talk's state and subtitles from `TalkEvent`s, starts/stops it over RPC, and — for a
// relay-linked client (§5.3) — streams the mic in and plays the Opus leg out through `speech.ts`.

import { useSyncExternalStore } from 'react'
import { toast } from 'sonner'
import { closeTap, openTap, type Tap, wireTap } from '@/lib/mic-tap'
import * as speech from '@/lib/speech'
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

/** The two subtitle lines and the utterance the agent line belongs to (cleared by its `SpeechFinished`). */
export type Subtitles = { agent: string; speech: string | null; user: string; utts: string[] }

export const NO_SUBTITLES: Subtitles = { agent: '', speech: null, user: '', utts: [] }

/** The string 53 shows and `Degraded { mic_no_aec }` means. */
export const NO_AEC = 'No echo cancellation; headphones recommended'

/**
 * Subtitle reducer (§4 of the brief). Agent: `Saying` text until the matching `SpeechFinished` or `Interrupted`.
 * User: `Heard` partials per utterance, append-only (a shorter partial never replaces a longer one, so the line
 * never flickers), the final replaces, `Submitted` clears the turn.
 */
export function applySubtitles(s: Subtitles, e: TalkEvent | Extract<VoiceEvent, { type: 'SpeechFinished' }>): Subtitles {
  switch (e.type) {
    case 'Saying':
      return { ...s, agent: e.text, speech: e.speech }
    case 'SpeechFinished':
      return s.speech === e.id ? { ...s, agent: '', speech: null } : s
    case 'Interrupted':
      return { ...s, agent: '', speech: null }
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
    void speech.release('talk')
  }
  speech.setTalk(null)
}

/** Reducer over the events; exported for the simulator, which feeds the same path the bus does. */
export function handle(e: TalkEvent | Extract<VoiceEvent, { type: 'SpeechFinished' }>): void {
  if (e.type === 'SpeechFinished') {
    set({ ...talk, subs: applySubtitles(talk.subs, e) })
    return
  }
  // Not ours: another client started one, or `TalkFinished` for a talk we never saw.
  if (talk.id !== null && e.id !== talk.id && e.type !== 'TalkStarted') return
  switch (e.type) {
    case 'TalkStarted':
      set({ ...talk, phase: 'on', id: e.id, session: e.session, aec: e.aec, state: 'listening', subs: NO_SUBTITLES })
      return
    case 'TalkState':
      set({ ...talk, state: e.state })
      return
    case 'Degraded':
      set({ ...talk, degraded: NO_AEC })
      return
    case 'Flush':
      if (talk.mine) speech.stop()
      return
    case 'TalkFinished':
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
  let id: string | null = null
  try {
    const t = remote ? await openTap('talk') : null
    tap = t
    if (t) speech.setTalk(session)
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
