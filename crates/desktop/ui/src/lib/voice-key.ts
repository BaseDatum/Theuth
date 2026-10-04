// 96 §5: the voice key in the island webview. The Rust shell owns the key, the clock and the overlays (`point::machine`)
// and tells us through `point:*` events; this module mirrors its phases into a zustand store, drives the dictation
// stream (`lib/dictation.ts`), asks the daemon for the screen (`screen_context`), sends the note to theuth's desk and
// follows the desk's turn (status line) and this webview's playback of it (subtitles; `speech.speakBlock` fires here
// like any other). Pure parts (`reduce`, `composeBody`, `sendMethod`, `statusLine`) are tested; `install()` wires the events.

import { create } from 'zustand'
import * as dictation from '@/lib/dictation'
import * as speech from '@/lib/speech'
import {
  type Annotate,
  type PointSample,
  islandPlace,
  onPointCancel,
  onPointLock,
  onPointRefused,
  onPointStart,
  onPointStop,
  onVoiceKeyPlace,
  pointCancel,
  pointDone,
  pointState,
  voiceKeyPlaced,
} from '@/lib/tauri'
import { useMeetingSettings } from '@/lib/meeting-settings'
import { trailColor } from '@/lib/trail'
import { noteText } from '@/lib/voice-note'
import { LOCAL, useEngines } from '@/link/engines'
import { useConnection } from '@/store/connection'
import { type Activity, on, useSessions } from '@/store/sessions'
import type { Api } from '@/rpc/api'

// ---- settings (§5.4; localStorage + zustand, `lib/nudges.ts` pattern) ----

const SETTINGS_KEY = 'voiceKey'
export type Accessibility = 'auto' | 'off'
export type Settings = {
  /** Off → the main webview binds `""`, which unregisters the key. */
  enabled: boolean
  includeScreen: boolean
  accessibility: Accessibility
  /** Times the "Hold to talk · double-tap to lock" hint was shown; shown on the first `HINT_USES`. */
  hintsShown: number
  /** The trail: the whole hold, or only while the left mouse button is held (`point_configure`). */
  annotate: Annotate
}
const DEFAULTS: Settings = { enabled: true, includeScreen: true, accessibility: 'auto', hintsShown: 0, annotate: 'always' }
export const HINT_USES = 3

function loadSettings(): Settings {
  try {
    return { ...DEFAULTS, ...(JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? '{}') as Partial<Settings>) }
  } catch {
    return DEFAULTS
  }
}

export const useVoiceKeySettings = create<Settings & { set(patch: Partial<Settings>): void }>()((set, get) => ({
  ...loadSettings(),
  set(patch) {
    const { set: _set, ...cur } = get()
    const next = { ...cur, ...patch }
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(next))
    set(next)
  },
}))

// ---- panel state (§5.1) ----

/**
 * `working` = the desk is on it (status line, strip); `speaking` = this webview plays a block (subtitles, grown);
 * `done` = the reply is over (`DONE_MS`); `error` = the desk's `Error` (`ERROR_MS`); `placing` = Settings' "Show panel
 * to move it" (a draggable strip, no gesture).
 */
export type Phase = 'idle' | 'listening' | 'sending' | 'working' | 'speaking' | 'done' | 'hint' | 'error' | 'placing'

export type State = {
  phase: Phase
  locked: boolean
  /** `Date.now()` at `point:start`; the listening clock. */
  startedAt: number
  durationMs: number
  /** The final transcript (after `dictation_stop`); live words come from `dictation.useDictation()` while listening. */
  text: string
  samples: PointSample[]
  lines: string[]
  files: string[]
  desk: string | null
  /** Accumulated `TextDelta` of the desk's turn (the subtitles' fallback). */
  reply: string
  error: string | null
  /** Shown in the listening slot: a cancel hint, a refusal, "Nothing recognized", … */
  hint: string | null
  /** An `ApprovalRequested` of the desk is unresolved. */
  approval: boolean
  /** The assistant entry last played here (`done` shows its last sentence). */
  spoken: string | null
}

export const IDLE: State = {
  phase: 'idle',
  locked: false,
  startedAt: 0,
  durationMs: 0,
  text: '',
  samples: [],
  lines: [],
  files: [],
  desk: null,
  reply: '',
  error: null,
  hint: null,
  approval: false,
  spoken: null,
}

export type Event =
  | { type: 'start'; at: number }
  | { type: 'lock' }
  | { type: 'stop'; samples: PointSample[]; durationMs: number }
  | { type: 'text'; text: string }
  | { type: 'context'; lines: string[]; files: string[] }
  | { type: 'sent'; desk: string }
  | { type: 'delta'; text: string }
  | { type: 'approval'; on: boolean }
  /** The desk's `Error`: red text for `ERROR_MS`, then `point_cancel()`. */
  | { type: 'error'; message: string }
  /** The desk started an utterance (engine `SpeechStarted`): a block is on its way — leave `done`. */
  | { type: 'queued' }
  /** This webview plays `entry` (`speech.useSpeaking()` set). */
  | { type: 'playing'; entry: string }
  /** Playback ended here (`speech.useSpeaking()` cleared); the turn may still be running. */
  | { type: 'silent' }
  /** The reply is over: the turn finished and nothing plays. */
  | { type: 'spoken' }
  /** A refusal or a failure: `hint` for `HINT_MS`, then idle. `null` → idle at once. */
  | { type: 'cancel'; hint: string | null }
  | { type: 'idle' }
  /** Settings › Voice key "Show panel to move it" / its Done. */
  | { type: 'place' }
  | { type: 'placed' }

/** The reply's phases: the desk's events and this webview's playback belong to the panel. */
export const replying = (p: Phase): boolean => p === 'working' || p === 'speaking' || p === 'done' || p === 'error'

/** Pure: the panel's phase and fields after `e`. */
export function reduce(s: State, e: Event): State {
  switch (e.type) {
    case 'start':
      return { ...IDLE, phase: 'listening', startedAt: e.at }
    case 'lock':
      return s.phase === 'listening' ? { ...s, locked: true } : s
    case 'stop':
      return { ...s, phase: 'sending', samples: e.samples, durationMs: e.durationMs }
    case 'text':
      return { ...s, text: e.text }
    case 'context':
      return { ...s, lines: e.lines, files: e.files }
    case 'sent':
      return { ...s, phase: 'working', desk: e.desk, reply: '', error: null, hint: null, approval: false, spoken: null }
    case 'delta':
      return replying(s.phase) ? { ...s, reply: s.reply + e.text } : s
    case 'approval':
      return replying(s.phase) ? { ...s, approval: e.on } : s
    case 'error':
      return replying(s.phase) ? { ...s, phase: 'error', error: e.message } : s
    case 'queued':
      return s.phase === 'done' ? { ...s, phase: 'working' } : s
    case 'playing':
      return s.phase === 'working' || s.phase === 'done' ? { ...s, phase: 'speaking', spoken: e.entry } : s
    case 'silent':
      return s.phase === 'speaking' ? { ...s, phase: 'working' } : s
    case 'spoken':
      return s.phase === 'working' ? { ...s, phase: 'done' } : s
    case 'cancel':
      return e.hint === null ? IDLE : { ...IDLE, phase: 'hint', hint: e.hint }
    case 'idle':
      return IDLE
    case 'place':
      return s.phase === 'idle' ? { ...IDLE, phase: 'placing' } : s
    case 'placed':
      return s.phase === 'placing' ? IDLE : s
  }
}

/** The `working` row: what the desk is doing, from its slice (`store/sessions.ts`) and the pending approval. */
export function statusLine(slice: { waiting: boolean; activity: Activity } | undefined, tool: string | undefined, approval: boolean): string {
  if (approval) return 'Needs your approval'
  if (!slice || slice.waiting) return 'Thinking…'
  switch (slice.activity) {
    case 'reasoning':
      return 'Reasoning…'
    case 'tool':
      return tool ? `Running ${tool}…` : 'Working…'
    case 'vision':
      return 'Looking at the screen…'
    case 'streaming':
      return 'Writing…'
    case 'thinking':
      return 'Thinking…'
    default:
      return 'Working…'
  }
}

/** `done`'s strip: the last sentence of what was spoken. */
export function lastSentence(text: string): string {
  const parts = text.trim().split(/(?<=[.!?])\s+/)
  return parts[parts.length - 1] ?? ''
}

/** §0.11: the voice-note line first, then one `[screen …]` line per visited display. */
export function composeBody(text: string, durationMs: number, lines: string[]): string {
  const note = noteText(text.trim(), durationMs)
  return lines.length ? `${note}\n${lines.join('\n')}` : note
}

/** `Composer.tsx:676-686`: a running desk is steered, an idle one gets `send_input`. */
export const sendMethod = (running: boolean): 'steer' | 'send_input' => (running ? 'steer' : 'send_input')

/** The desk is opened once per engine (§0.12); the cache key. */
export const deskCacheKey = (engine: string, name = 'theuth'): string => `${engine}/${name}`

export const DEFAULT_HINT = 'Hold to talk · double-tap to lock'
export const MOUSE_HINT = 'Hold the left mouse button to draw'
/** `done` stays this long after the reply is over ("once the agent is idle again the window goes away"). */
export const DONE_MS = 2000
/** A hint or refusal in the listening slot. */
export const HINT_MS = 2000
/** The desk's `Error` in red, then `point_cancel()`. */
export const ERROR_MS = 4000
/** The wait for the dictation's final flush before sending what we have (spike 6 measures the real figure). */
const FINAL_TEXT_MS = 8000
/**
 * Turn finished and nothing plays: a queued block takes one `speak` round trip before `speech.useSpeaking()` is set
 * (`enqueue` → `idle` → `speak`), so the reply counts as over only after this grace. Nothing to say / voice off → done.
 */
export const SPEAK_WAIT_MS = 1500

export const useVoiceKey = create<State>()(() => IDLE)

export function dispatch(e: Event): void {
  useVoiceKey.setState(reduce(useVoiceKey.getState(), e))
}

// ---- orchestration ----

const desks = new Map<string, string>()
let hintTimer = 0
let doneTimer = 0
let errorTimer = 0
let waitTimer = 0
/** `dictation.start` in flight: a stop that lands first waits for the id. */
let starting: Promise<string | null> | null = null
/** The desk's `TurnFinished` arrived (reset by `TurnStarted` and every press). */
let turnDone = false
/** `speech.useSpeaking()` is set in this webview (fed by the panel through `playback`). */
let playing = false
/** The desk's unresolved approvals (`ApprovalRequested` ids). */
const approvals = new Set<string>()

function clearTimers(): void {
  clearTimeout(hintTimer)
  clearTimeout(doneTimer)
  clearTimeout(errorTimer)
  clearTimeout(waitTimer)
  hintTimer = doneTimer = errorTimer = waitTimer = 0
}

/**
 * Show `hint` (or nothing) and return to idle; the shell is told through `tell`. Leaves the desk's voice mode:
 * a typed message in the desk chat afterwards gets a typed, unspoken reply (this webview would otherwise read it).
 */
function finish(hint: string | null, tell: () => Promise<void>): void {
  clearTimers()
  const { desk } = useVoiceKey.getState()
  if (desk) speech.setVoiceMode(desk, false)
  void tell().catch(() => undefined)
  dispatch({ type: 'cancel', hint })
  if (hint !== null) hintTimer = window.setTimeout(() => dispatch({ type: 'idle' }), HINT_MS)
}

async function onStart(at: number): Promise<void> {
  clearTimers()
  const prev = useVoiceKey.getState()
  if (prev.phase === 'placing') void voiceKeyPlaced().catch(() => undefined)
  // A press mid-reply asks the next question: the old answer stops so the mic does not hear it.
  if (replying(prev.phase)) stopSpeech(prev.desk)
  turnDone = false
  approvals.clear()
  dispatch({ type: 'start', at })
  const settings = useVoiceKeySettings.getState()
  if (settings.hintsShown < HINT_USES) settings.set({ hintsShown: settings.hintsShown + 1 })
  const api = useConnection.getState().api
  if (!api) return finish('Not connected', pointCancel)
  // `Composer.tsx:374`: a LOCAL engine captures natively; a linked one streams this webview's mic.
  starting = dictation.start(api, '', '', useEngines.getState().active !== LOCAL, true)
  const refused = await starting
  starting = null
  // The engine answers "already dictating" / "talking" (voice/src/dictation.rs) — a local hint, no `point_refuse`.
  if (refused !== null && useVoiceKey.getState().phase === 'listening') finish(refused, pointCancel)
}

function stopSpeech(desk: string | null): void {
  speech.stop()
  if (desk) void useConnection.getState().api?.stop_speaking(null).catch(() => undefined)
}

/** `dictation_stop`, then the final text from `DictationFinished` (spike 6: the delta is logged); what we have on timeout. */
async function finalText(api: Api): Promise<string> {
  await starting
  if (!dictation.current().active?.id) return ''
  const t0 = performance.now()
  const { promise, resolve } = Promise.withResolvers<string>()
  const off = dictation.onFinish((r) => resolve(r.value))
  // ponytail: a flush that never lands sends the open utterances as they stand; the engine's `Finished` is then ignored.
  const timer = window.setTimeout(() => resolve(dictation.splice('', '', dictation.current().active?.utts ?? []).value), FINAL_TEXT_MS)
  await dictation.stop(api)
  const text = await promise
  off()
  clearTimeout(timer)
  console.info(`voice-key: dictation_stop → final text in ${Math.round(performance.now() - t0)} ms`)
  return text
}

async function onStop(samples: PointSample[], durationMs: number): Promise<void> {
  dispatch({ type: 'stop', samples, durationMs })
  const api = useConnection.getState().api
  if (!api) return finish('Not connected', pointCancel)
  const settings = useVoiceKeySettings.getState()
  const screen = settings.includeScreen
    ? api
        .screen_context({ samples, color: trailColor(), since_ms: durationMs, accessibility: settings.accessibility !== 'off' })
        .catch((e: unknown) => {
          console.warn('screen_context', e)
          useVoiceKey.setState({ hint: 'Screenshots off — sent without the screen' })
          return { lines: [], files: [] }
        })
    : Promise.resolve({ lines: [], files: [] })
  const [text, ctx] = await Promise.all([finalText(api), screen])
  if (useVoiceKey.getState().phase !== 'sending') return
  dispatch({ type: 'text', text })
  dispatch({ type: 'context', lines: ctx.lines, files: ctx.files })
  if (!text.trim()) return finish('Nothing recognized', pointCancel)
  const engine = useEngines.getState().active
  let desk = desks.get(deskCacheKey(engine))
  try {
    if (!desk) {
      desk = await api.desk_open('theuth')
      desks.set(deskCacheKey(engine), desk)
    }
  } catch (e) {
    console.warn('desk_open', e)
    return finish("theuth's desk lives on another engine", pointCancel)
  }
  const body = composeBody(text, durationMs, ctx.lines)
  const clientId = crypto.randomUUID()
  speech.setVoiceMode(desk, true)
  try {
    // The slice is built from this webview's event stream; an unseen desk reads as idle.
    const running = useSessions.getState().sessions[desk]?.running ?? false
    if (sendMethod(running) === 'steer') await api.steer(desk, body, clientId, ctx.files)
    else await api.send_input(desk, body, ctx.files, null, clientId)
  } catch (e) {
    speech.setVoiceMode(desk, false)
    return finish(e instanceof Error ? e.message : String(e), pointCancel)
  }
  dispatch({ type: 'sent', desk })
  void pointDone().catch(() => undefined)
}

/** Reply over (turn finished, nothing plays): `done` for `DONE_MS`, then the shell's `PanelDone` → idle. */
function onSpoken(): void {
  dispatch({ type: 'spoken' })
  clearTimeout(doneTimer)
  doneTimer = window.setTimeout(() => finish(null, pointDone), DONE_MS)
}

/** The reply is over when BOTH hold: the desk's turn finished and nothing plays here — after `SPEAK_WAIT_MS` for a block on its way. */
function settle(): void {
  clearTimeout(waitTimer)
  if (!turnDone || playing) return
  waitTimer = window.setTimeout(() => {
    if (turnDone && !playing && useVoiceKey.getState().phase === 'working') onSpoken()
  }, SPEAK_WAIT_MS)
}

/** The desk's turn started / finished (`TurnStarted` / `TurnFinished`). */
export function turn(finished: boolean): void {
  turnDone = finished
  if (finished) settle()
  else clearTimeout(waitTimer)
}

/** This webview's playback (`speech.useSpeaking()`, fed by the panel): `working ↔ speaking`, and the reply's end. */
export function playback(entry: string | null): void {
  playing = entry !== null
  if (entry !== null) {
    clearTimeout(waitTimer)
    clearTimeout(doneTimer)
    dispatch({ type: 'playing', entry })
  } else {
    dispatch({ type: 'silent' })
    settle()
  }
}

/** Esc / Stop: cut the speech and tell the shell to cancel; while placing, Esc is Done. */
export function cancel(): void {
  const { phase, desk } = useVoiceKey.getState()
  if (phase === 'idle') return
  if (phase === 'placing') return donePlacing()
  if (phase === 'listening') {
    const api = useConnection.getState().api
    if (api) void dictation.discard(api)
  }
  if (replying(phase)) stopSpeech(desk)
  finish(null, pointCancel)
}

/** Settings' Done (or the panel's): hide, tell Settings. */
export function donePlacing(): void {
  dispatch({ type: 'placed' })
  void voiceKeyPlaced().catch(() => undefined)
}

/** "Reset to default": place at the notch default and forget this layout's saved spot (the key is what `island_place` answers). */
async function resetPlace(): Promise<void> {
  const [, , key] = await islandPlace({})
  useMeetingSettings.getState().clearPosition(key)
}

/** Seed from the shell's machine (the island webview may boot mid-gesture): join a hold, or clear a phase we cannot resume. */
export async function seed(): Promise<void> {
  const s = await pointState().catch(() => null)
  if (!s || s.state === 'idle' || useVoiceKey.getState().phase !== 'idle') return
  // Mid-send or mid-reply the context is gone with the old webview: cancel (`Sending`/`Replying` + Cancel → Idle).
  if (s.state === 'sending' || s.state === 'replying') void pointCancel().catch(() => undefined)
  else await onStart(Date.now())
}

/**
 * Wire `point:*`, Settings' place events and the desk's events; once per island webview boot. Returns the unsubscribe.
 * The reply ends when the turn has finished AND playback here is idle (`turn`, `playback`); the engine's
 * `SpeechFinished` says only that synthesis is done and plays no part.
 */
export function install(): () => void {
  const offs = [
    onPointStart(() => void onStart(Date.now())),
    onPointLock(() => dispatch({ type: 'lock' })),
    onPointStop((p) => void onStop(p.samples, p.duration_ms)),
    onPointCancel((p) => {
      const { phase } = useVoiceKey.getState()
      // Our own `point_cancel` (a refusal already on screen) echoes back without a hint: keep showing ours.
      if ((phase === 'hint' && p.hint === null) || phase === 'placing') return
      const api = useConnection.getState().api
      if (api && phase === 'listening') void dictation.discard(api)
      finish(p.hint, () => Promise.resolve())
    }),
    onPointRefused((p) => finish(p.reason === 'meeting' ? 'Meeting running — use ⌥` to ask' : p.reason, () => Promise.resolve())),
    onVoiceKeyPlace((p) => {
      if (p.reset) void resetPlace().catch((e: unknown) => console.warn('island_place', e))
      else if (p.on) dispatch({ type: 'place' })
      else donePlacing()
    }),
  ]
  const mine = (session: string) => {
    const s = useVoiceKey.getState()
    return s.desk === session && replying(s.phase)
  }
  const sessionOffs = [
    on('TextDelta', (e) => mine(e.session) && dispatch({ type: 'delta', text: e.text })),
    on('ApprovalRequested', (e) => {
      if (!mine(e.session)) return
      approvals.add(e.id)
      dispatch({ type: 'approval', on: true })
    }),
    on('ApprovalResolved', (e) => {
      if (approvals.delete(e.id) && approvals.size === 0) dispatch({ type: 'approval', on: false })
    }),
    on('Error', (e) => {
      if (!mine(e.session)) return
      dispatch({ type: 'error', message: e.message })
      clearTimers()
      errorTimer = window.setTimeout(() => {
        stopSpeech(e.session)
        finish(null, pointCancel)
      }, ERROR_MS)
    }),
    on('TurnStarted', (e) => mine(e.session) && turn(false)),
    on('TurnFinished', (e) => mine(e.session) && turn(true)),
    // Synthesis started: playback follows (or has begun) — a `done` panel comes back as `working`; the grace restarts.
    on('SpeechStarted', (e) => {
      if (!mine(e.session)) return
      clearTimeout(doneTimer)
      dispatch({ type: 'queued' })
      settle()
    }),
  ]
  return () => {
    clearTimers()
    for (const u of offs) void u.then((f) => f())
    for (const off of sessionOffs) off()
  }
}
