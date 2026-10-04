// 96 §5: the voice key in the island webview. The Rust shell owns the key, the clock and the overlays (`point::machine`)
// and tells us through `point:*` events; this module mirrors its phases into a zustand store, drives the dictation
// stream (`lib/dictation.ts`), asks the daemon for the screen (`screen_context`), sends the note to theuth's desk and
// follows the desk's turn for subtitles and speech (`speech.speakBlock` fires in this webview like any other).
// Pure parts (`reduce`, `composeBody`, `sendMethod`) are tested; `install()` wires the events.

import { create } from 'zustand'
import * as dictation from '@/lib/dictation'
import * as speech from '@/lib/speech'
import {
  type PointSample,
  onPointCancel,
  onPointLock,
  onPointRefused,
  onPointStart,
  onPointStop,
  pointCancel,
  pointDone,
  pointState,
} from '@/lib/tauri'
import { trailColor } from '@/lib/trail'
import { noteText } from '@/lib/voice-note'
import { LOCAL, useEngines } from '@/link/engines'
import { useConnection } from '@/store/connection'
import { on, useSessions } from '@/store/sessions'
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
}
const DEFAULTS: Settings = { enabled: true, includeScreen: true, accessibility: 'auto', hintsShown: 0 }
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

export type Phase = 'idle' | 'listening' | 'sending' | 'replying' | 'done' | 'hint'

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
  /** Accumulated `TextDelta` of the desk's turn. */
  reply: string
  error: string | null
  /** Shown in the listening slot: a cancel hint, a refusal, "Nothing recognized", … */
  hint: string | null
  approval: boolean
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
}

export type Event =
  | { type: 'start'; at: number }
  | { type: 'lock' }
  | { type: 'stop'; samples: PointSample[]; durationMs: number }
  | { type: 'text'; text: string }
  | { type: 'context'; lines: string[]; files: string[] }
  | { type: 'sent'; desk: string }
  | { type: 'delta'; text: string }
  | { type: 'approval' }
  | { type: 'error'; message: string }
  /** An utterance of the desk started: back from `done` to `replying` (a later block after a gap). */
  | { type: 'speaking' }
  /** The last spoken block ended (or there was nothing to speak). */
  | { type: 'spoken' }
  /** A refusal or a failure: `hint` for `HINT_MS`, then idle. `null` → idle at once. */
  | { type: 'cancel'; hint: string | null }
  | { type: 'idle' }

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
      return { ...s, phase: 'replying', desk: e.desk, reply: '', error: null, hint: null, approval: false }
    case 'delta':
      return s.phase === 'replying' || s.phase === 'done' ? { ...s, reply: s.reply + e.text } : s
    case 'approval':
      return { ...s, approval: true }
    case 'error':
      return { ...s, error: e.message }
    case 'speaking':
      return s.phase === 'done' ? { ...s, phase: 'replying' } : s
    case 'spoken':
      return s.phase === 'replying' ? { ...s, phase: 'done' } : s
    case 'cancel':
      return e.hint === null ? IDLE : { ...IDLE, phase: 'hint', hint: e.hint }
    case 'idle':
      return IDLE
  }
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
/** `done` stays this long after the last `SpeechFinished`. */
export const DONE_MS = 6000
/** A hint or refusal in the listening slot. */
export const HINT_MS = 2000
/** The wait for the dictation's final flush before sending what we have (spike 6 measures the real figure). */
const FINAL_TEXT_MS = 8000
/** After `TurnFinished`, how long a `SpeechStarted` may take before the reply counts as spoken (nothing to say, voice off). */
const SPEAK_WAIT_MS = 4000

export const useVoiceKey = create<State>()(() => IDLE)

export function dispatch(e: Event): void {
  useVoiceKey.setState(reduce(useVoiceKey.getState(), e))
}

// ---- orchestration ----

const desks = new Map<string, string>()
let hintTimer = 0
let doneTimer = 0
/** `dictation.start` in flight: a stop that lands first waits for the id. */
let starting: Promise<string | null> | null = null

function clearTimers(): void {
  clearTimeout(hintTimer)
  clearTimeout(doneTimer)
  hintTimer = doneTimer = 0
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

/** Replying: after the last block is spoken, `DONE_MS`, then the shell's `PanelDone` → idle. */
function onSpoken(): void {
  dispatch({ type: 'spoken' })
  clearTimeout(doneTimer)
  doneTimer = window.setTimeout(() => finish(null, pointDone), DONE_MS)
}

/** Esc / click outside / Stop: cut the speech and tell the shell to cancel. */
export function cancel(): void {
  const { phase, desk } = useVoiceKey.getState()
  if (phase === 'idle') return
  if (phase === 'listening') {
    const api = useConnection.getState().api
    if (api) void dictation.discard(api)
  }
  if (phase === 'replying' || phase === 'done') {
    speech.stop()
    if (desk) void useConnection.getState().api?.stop_speaking(null).catch(() => undefined)
  }
  finish(null, pointCancel)
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
 * Wire `point:*` and the desk's events; once per island webview boot. Returns the unsubscribe.
 * The reply ends at `SpeechFinished` of the last utterance after `TurnFinished`; a turn that starts no speech
 * within `SPEAK_WAIT_MS` (nothing to say, voice plugin off) ends it too.
 */
export function install(): () => void {
  const offs = [
    onPointStart(() => void onStart(Date.now())),
    onPointLock(() => dispatch({ type: 'lock' })),
    onPointStop((p) => void onStop(p.samples, p.duration_ms)),
    onPointCancel((p) => {
      const { phase } = useVoiceKey.getState()
      // Our own `point_cancel` (a refusal already on screen) echoes back without a hint: keep showing ours.
      if (phase === 'hint' && p.hint === null) return
      const api = useConnection.getState().api
      if (api && phase === 'listening') void dictation.discard(api)
      finish(p.hint, () => Promise.resolve())
    }),
    onPointRefused((p) => finish(p.reason === 'meeting' ? 'Meeting running — use ⌥` to ask' : p.reason, () => Promise.resolve())),
  ]
  const mine = (session: string) => {
    const s = useVoiceKey.getState()
    return s.desk === session && (s.phase === 'replying' || s.phase === 'done')
  }
  /** The desk's utterances in flight (`SpeechStarted` → `SpeechFinished`). */
  const utterances = new Set<string>()
  let finished = false
  let waitTimer = 0
  const sessionOffs = [
    on('TextDelta', (e) => mine(e.session) && dispatch({ type: 'delta', text: e.text })),
    on('ApprovalRequested', (e) => mine(e.session) && dispatch({ type: 'approval' })),
    on('Error', (e) => mine(e.session) && dispatch({ type: 'error', message: e.message })),
    on('TurnStarted', (e) => {
      if (mine(e.session)) finished = false
    }),
    on('TurnFinished', (e) => {
      if (!mine(e.session)) return
      finished = true
      clearTimeout(waitTimer)
      waitTimer = window.setTimeout(() => utterances.size === 0 && onSpoken(), SPEAK_WAIT_MS)
    }),
    on('SpeechStarted', (e) => {
      if (!mine(e.session)) return
      utterances.add(e.id)
      clearTimeout(waitTimer)
      clearTimeout(doneTimer)
      dispatch({ type: 'speaking' })
    }),
    on('SpeechFinished', (e) => {
      if (!utterances.delete(e.id)) return
      if (finished && utterances.size === 0) onSpoken()
    }),
  ]
  return () => {
    clearTimers()
    clearTimeout(waitTimer)
    for (const u of offs) void u.then((f) => f())
    for (const off of sessionOffs) off()
  }
}
