// 54 §5: live dictation into the composer. The daemon owns the model and, for the desktop, the mic;
// a relay-linked client streams its own mic instead (§4.5). This module keeps the caret anchor
// (`before`/`after`), the utterances the daemon sends, the lifecycle, and the browser capture graph,
// and hands the composer the spliced value when a dictation ends. Insert-only: `before` and `after`
// are never touched.

import { type RefObject, useEffect, useRef, useState, useSyncExternalStore } from 'react'
import { toast } from 'sonner'
import { closeTap, openTap, type Tap, wireTap } from '@/lib/mic-tap'
import { touch } from '@/lib/platform'
import * as speech from '@/lib/speech'
import type { Api } from '@/rpc/api'
import type { DeviceState } from '@/rpc/bindings/DeviceState'
import type { DictationOpts } from '@/rpc/bindings/DictationOpts'
import type { EndCommand } from '@/rpc/bindings/EndCommand'
import type { ModelState } from '@/rpc/bindings/ModelState'
import type { Surface } from '@/rpc/bindings/Surface'
import type { VoiceEvent } from '@/rpc/bindings/VoiceEvent'
import { on } from '@/store/sessions'

/**
 * One utterance as the daemon reports it; `final: false` renders muted. `shaped` is the written form (153 §4) the
 * field shows and sends; `command` a spoken end command armed on it (153 §7).
 */
export type Utt = { text: string; shaped: string; final: boolean; command?: EndCommand }
/** What the field shows for an utterance: the shaped text; the raw text only when shaping sent none and no end command consumed the words (an older engine). */
export const shown = (u: Utt): string => u.shaped || (u.command ? '' : u.text)
/** 153 §8.2: the mic re-armed on `name`, or was lost (reconnecting). */
export type Device = { state: DeviceState; name: string | null }
/** §5.2 lifecycle. */
export type Phase = 'idle' | 'starting' | 'dictating' | 'stopping'
/** The dictation this frontend started. `id` is `''` until `dictation_start` answers or `DictationStarted` arrives. */
export type Active = {
  id: string
  before: string
  after: string
  utts: Utt[]
  /** Last 24 RMS values (20 ms each) for the meter. */
  levels: number[]
  startedAt: number
  /** The last `DictationDevice` of this dictation; `null` while the device that opened still runs. */
  device: Device | null
}
/** Store shape; `elsewhere` = another frontend's dictation holds the mic. */
export type State = {
  model: ModelState
  active: Active | null
  phase: Phase
  elsewhere: boolean
}

const LEVELS = 24
/** Splice result: the composer value and where the caret goes. */
export type Splice = { value: string; caret: number }

/** §5.1: `before + region + after`, caret after the last dictated word (before the trailing separator); unchanged when nothing was said. */
export function splice(before: string, after: string, utts: Utt[]): Splice {
  const body = utts
    .map(shown)
    .filter(Boolean)
    .join(' ')
  if (!body) return { value: before + after, caret: before.length }
  const lead = before && !/\s$/.test(before) ? ' ' : ''
  const trail = after && !/^[\s.,;:!?)]/.test(after) ? ' ' : ''
  const region = lead + body + trail
  return { value: before + region + after, caret: before.length + lead.length + body.length }
}

/** Pure reducer over the `Dictation*` events; ids not ours only move `elsewhere`. */
export function applyEvent(s: State, e: VoiceEvent): State {
  switch (e.type) {
    case 'DictationModel':
      return { ...s, model: e.state }
    case 'DictationStarted': {
      const a = s.active
      if (!a) return { ...s, elsewhere: true }
      // Ours: the id we got back, or the one still pending because the event beat the RPC reply.
      if (a.id === e.id || (a.id === '' && s.phase === 'starting'))
        return { ...s, phase: 'dictating', active: { ...a, id: e.id } }
      return s
    }
    case 'DictationLevel':
      if (s.active?.id !== e.id) return s
      return { ...s, active: { ...s.active, levels: [...s.active.levels, ...e.levels].slice(-LEVELS) } }
    case 'DictationText': {
      if (s.active?.id !== e.id) return s
      const utts = s.active.utts.slice()
      while (utts.length < e.utterance) utts.push({ text: '', shaped: '', final: true })
      // "scratch that" (153 §7.4) arrives as an empty final for the EARLIER utterance: the same write empties it.
      const u: Utt = { text: e.text, shaped: e.shaped, final: e.final }
      if (e.command) u.command = e.command
      utts[e.utterance] = u
      return { ...s, active: { ...s.active, utts } }
    }
    case 'DictationDevice': {
      if (s.active?.id !== e.id) return s
      return { ...s, active: { ...s.active, device: { state: e.state, name: e.device } } }
    }
    case 'DictationFinished':
      if (s.active?.id === e.id) return { ...s, active: null, phase: 'idle' }
      return s.elsewhere ? { ...s, elsewhere: false } : s
    default:
      return s
  }
}

let state: State = { model: { state: 'loading' }, active: null, phase: 'idle', elsewhere: false }
/** Esc pressed: the finish hands back `before + after` instead of the region. */
let discarding = false
const subs = new Set<() => void>()
const finishers = new Set<(r: Finish) => void>()

/**
 * What `onFinish` hands the surface: the splice, the dictation's id (for `correction`; `''` when Esc discarded it —
 * §6.1 reports nothing for a discard), and whether "send it" was said.
 */
export type Finish = Splice & { id: string; send: boolean }

/**
 * The value a finished dictation lands (153 §7.4): Esc → `before + after`; else the shaped splice. `send` when a
 * closed utterance carries the `send` command — a partial that only armed it never sends.
 */
export function finish(a: Active, discard: boolean): Finish {
  if (discard) return { value: a.before + a.after, caret: a.before.length, id: '', send: false }
  return { ...splice(a.before, a.after, a.utts), id: a.id, send: a.utts.some((u) => u.final && u.command?.command === 'send') }
}

/**
 * 153 §6.3: a dictionary suggestion the engine published. `reporter` is the token the sending surface gave `correction`
 * (its card shows it); `dictationId` the dictation that report named — the event carries none, and Play plays its audio.
 */
export type Suggestion = {
  id: string
  heard: string
  typed: string
  occurrences: number
  hasAudio: boolean
  reporter: object | null
  dictationId: string | null
}
let suggestion: Suggestion | null = null
/** The last `correction` report: who sent, and which dictation (§6.3 step 4). */
let reporter: { by: object; dictationId: string } | null = null
const suggestionSubs = new Set<() => void>()
const suggestionSnapshot = () => suggestion
const subscribeSuggestion = (cb: () => void) => {
  suggestionSubs.add(cb)
  return () => {
    suggestionSubs.delete(cb)
  }
}
/** The pending suggestion card, re-rendering on change; `null` once answered or dismissed. */
export function useSuggestion(): Suggestion | null {
  return useSyncExternalStore(subscribeSuggestion, suggestionSnapshot, suggestionSnapshot)
}
/** Drop the card locally (after `dictionary_suggestion_answer`, or the 20 s auto-dismiss). */
export function clearSuggestion(id: string): void {
  if (suggestion?.id !== id) return
  suggestion = null
  for (const cb of suggestionSubs) cb()
}

/**
 * 153 §6.1: report what was actually sent after dictation `id`, fire-and-forget; the engine diffs and may answer with a
 * `DictionarySuggestion`, shown by the surface that passed `by`.
 */
export function correction(api: Api, id: string, sentText: string, by: object): void {
  if (!id || !sentText) return
  reporter = { by, dictationId: id }
  void api.dictation_correction(id, sentText).catch(() => undefined)
}

/** The browser-side mic graph of a remote-source dictation (54 §4.5); `null` while the daemon owns the mic. */
let capture: Tap | null = null
/** 82 §B: raw `DictationLevel` packets of our dictation, for the waveform when there is no `capture` (local engine). */
const levelSubs = new Set<(levels: number[]) => void>()

/** The mic graph of the running dictation, if this browser captures it (remote engine); else `null`. */
export const tap = (): Tap | null => capture

/** Each level packet (eight 20 ms RMS values, oldest first) of our dictation. Returns the unsubscribe. */
export function onLevels(cb: (levels: number[]) => void): () => void {
  levelSubs.add(cb)
  return () => {
    levelSubs.delete(cb)
  }
}

/**
 * Release the mic, the tap nodes and speech's `dictate` hold (the session suspends once nothing else holds
 * it); idempotent, and run on every end — a failed `getUserMedia` has no capture yet still took the hold.
 */
function teardownCapture(): void {
  const c = capture
  capture = null
  if (c) closeTap(c)
  void speech.release('dictate')
}

function set(next: State): void {
  if (next === state) return
  state = next
  for (const cb of subs) cb()
}

/** Current state via a call: TS pins a module `let`'s narrowing across awaits, so post-await reads go through here. */
function snapshot(): State {
  return state
}

const subscribe = (cb: () => void) => {
  subs.add(cb)
  return () => {
    subs.delete(cb)
  }
}

/** The store, re-rendering on change. */
export function useDictation(): State {
  return useSyncExternalStore(subscribe, snapshot, snapshot)
}

/** `dictation_prewarm`'s answer (App calls it on connect); events keep it current afterwards. */
export function setModel(model: ModelState): void {
  set({ ...state, model })
}

/** Why the button is disabled, or `null` when dictation can start. */
export function reason(s: State): string | null {
  const m = s.model
  if (m.state === 'loading') return 'Dictation model loading…'
  if (m.state === 'missing') return 'Dictation model missing'
  if (m.state === 'failed') {
    const path = m.message.match(/\S*\/\S*/)?.[0]
    return path ? `Dictation model missing (${path})` : `Dictation model failed: ${m.message}`
  }
  if (s.elsewhere) return 'Dictating elsewhere'
  return null
}

/**
 * One text box with a mic button (`components/ui/dictation.tsx`, SearchDialog): the store is global with one
 * `active`, so the box takes an `owner` flag exactly as `Composer.tsx` does and splices the finished text
 * at its caret (a selection is replaced). `shown` is the live spliced value while dictating; render it
 * read-only then. The field's DOM value wins over `value` at start, so uncontrolled fields work too.
 * `api === null` (not connected): the toggle is a no-op.
 */
export type Box = { dict: State; dictating: boolean; reason: string | null; toggle: () => void; shown: string }

export function useBox(
  api: Api | null,
  value: string,
  onChange: (v: string, caret: number) => void,
  ref: RefObject<HTMLInputElement | HTMLTextAreaElement | null>,
  mic: boolean,
): Box {
  const dict = useDictation()
  const [owner, setOwner] = useState(false)
  const ownerRef = useRef(owner)
  ownerRef.current = owner
  const dictating = owner && dict.active !== null
  const why = reason(dict) ?? (dict.active !== null && !owner ? 'Dictating in another box' : null)
  useEffect(
    () =>
      onFinish(({ value, caret }) => {
        if (ownerRef.current) onChange(value, caret)
      }),
    [onChange],
  )
  useEffect(() => {
    if (dict.phase === 'idle') setOwner(false)
  }, [dict.phase])
  const toggle = () => {
    if (!api) return
    if (dictating) {
      if (dict.phase === 'dictating') void stop(api)
      return
    }
    if (why) {
      toast(why)
      return
    }
    if (dict.phase !== 'idle') return
    const { before, after } = anchor(ref.current?.value ?? value, ref.current?.selectionStart ?? null, ref.current?.selectionEnd ?? null)
    setOwner(true)
    void start(api, before, after, mic)
  }
  const shown = dictating && dict.active ? splice(dict.active.before, dict.active.after, dict.active.utts).value : value
  return { dict, dictating, reason: why, toggle, shown }
}

/** The caret anchor: text before the selection and after it, so the dictated region replaces what was selected. No selection → the end. */
export function anchor(value: string, start: number | null, end: number | null): { before: string; after: string } {
  const s = start ?? value.length
  return { before: value.slice(0, s), after: value.slice(Math.max(s, end ?? s)) }
}

function handle(e: VoiceEvent): void {
  if (e.type === 'DictionarySuggestion') {
    suggestion = {
      id: e.id,
      heard: e.heard,
      typed: e.typed,
      occurrences: e.occurrences,
      hasAudio: e.has_audio,
      reporter: reporter?.by ?? null,
      dictationId: reporter?.dictationId ?? null,
    }
    for (const cb of suggestionSubs) cb()
    return
  }
  const prev = state
  const next = applyEvent(prev, e)
  set(next)
  if (e.type === 'DictationLevel' && prev.active?.id === e.id) for (const cb of levelSubs) cb(e.levels)
  if (e.type !== 'DictationFinished' || !prev.active || next.active) return
  teardownCapture()
  const r = finish(prev.active, discarding)
  discarding = false
  for (const cb of finishers) cb(r)
  if (e.reason.reason === 'error') toast.error(e.reason.message)
}

/** Wire the `Dictation*` events to the store; once per app boot. Returns the unsubscribe. */
export function installDictationListeners(): () => void {
  const offs = [
    on('DictationModel', handle),
    on('DictationStarted', handle),
    on('DictationLevel', handle),
    on('DictationText', handle),
    on('DictationDevice', handle),
    on('DictionarySuggestion', handle),
    on('DictationFinished', handle),
  ]
  return () => {
    for (const off of offs) off()
  }
}

/** Fired once when our dictation ends (daemon `Finished` after stop, discard, error or max length) with the value to put in the composer. */
export function onFinish(cb: (r: Finish) => void): () => void {
  finishers.add(cb)
  return () => {
    finishers.delete(cb)
  }
}

/** The store now, outside React (the voice-key orchestrator reads the open utterances and the active id). */
export const current = (): State => state

/** 153 §14.1: what `dictation_start` is told about this surface — the phone webview or the Composer, an agent prompt, the slash commands the box takes. */
export function startOpts(commands: string[], pressAgoMs: number): DictationOpts {
  const surface: Surface = touch() ? 'phone' : 'composer'
  return { surface, destination: { kind: 'agent_prompt' }, app: null, commands, press_ago_ms: Math.max(0, Math.round(pressAgoMs)) }
}

/**
 * Anchor the caret (`before`/`after`) and start dictating; a refusal toasts (unless `quiet`) and returns to idle.
 * `mic`: stream this browser's microphone to the daemon (a relay peer; 54 §4.5) instead of using the engine mic.
 * `commands`: the slash commands this surface accepts (153 §4.2 stage 4). Resolves with the refusal message, `null` when dictating.
 */
export async function start(api: Api, before: string, after: string, mic: boolean, quiet = false, commands: string[] = []): Promise<string | null> {
  if (state.active) return null
  const pressed = Date.now()
  set({
    ...state,
    phase: 'starting',
    active: { id: '', before, after, utts: [], levels: [], startedAt: pressed, device: null },
  })
  let id: string | null = null
  try {
    const c = mic ? await openTap('dictate') : null
    if (c) capture = c
    const sid = await api.dictation_start(c ? c.ctx.sampleRate : null, startOpts(commands, Date.now() - pressed))
    id = sid
    // `state` was narrowed to `active: null` above and TS keeps that across calls; read it fresh.
    const a = snapshot().active
    if (a?.id === '') set({ ...state, active: { ...a, id: sid } })
    // Mic → tap → 100 ms i16 batches → `dictation_audio`; a failed push is dropped.
    if (c) await wireTap(c, (b64) => void api.dictation_audio(sid, b64).catch(() => undefined), () => capture === c)
  } catch (e) {
    teardownCapture()
    // The daemon already holds the slot: hand it back; its `Finished` finds nothing active here and is ignored.
    if (id !== null) void api.dictation_stop(id).catch(() => undefined)
    discarding = false
    set({ ...state, phase: 'idle', active: null })
    const message = e instanceof Error ? e.message : String(e)
    if (!quiet) toast.error(message)
    void refreshStatus(api).catch(() => undefined)
    return message
  }
  return null
}

/** Stop and keep the region; the `DictationFinished` event completes the transition (and releases the mic). */
export async function stop(api: Api): Promise<void> {
  const a = state.active
  if (!a?.id || state.phase === 'stopping') return
  set({ ...state, phase: 'stopping' })
  // The last partial batch carries the tail of the final word.
  capture?.batcher?.flush()
  await api.dictation_stop(a.id).catch((e: unknown) => {
    teardownCapture()
    toast.error(e instanceof Error ? e.message : String(e))
  })
}

/** Esc: stop and restore `before + after` exactly; the mic closes at once, nothing more is sent. */
export function discard(api: Api): Promise<void> {
  if (state.active?.id) discarding = true
  teardownCapture()
  return stop(api)
}

/** `dictation_status` → model state and whether another frontend is dictating. */
export async function refreshStatus(api: Api): Promise<void> {
  const st = await api.dictation_status()
  set({ ...state, model: st.model, elsewhere: st.active !== null && st.active !== state.active?.id })
}
