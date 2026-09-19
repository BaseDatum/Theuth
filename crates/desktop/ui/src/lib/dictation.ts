// 54 §5: live dictation into the composer. The daemon owns the mic and the model; this module
// keeps the caret anchor (`before`/`after`), the utterances the daemon sends, and the lifecycle,
// and hands the composer the spliced value when a dictation ends. Insert-only: `before` and
// `after` are never touched.

import { useSyncExternalStore } from 'react'
import { toast } from 'sonner'
import type { Api } from '@/rpc/api'
import type { ModelState } from '@/rpc/bindings/ModelState'
import type { VoiceEvent } from '@/rpc/bindings/VoiceEvent'
import { on } from '@/store/sessions'

/** One utterance as the daemon reports it; `final: false` renders muted. */
export type Utt = { text: string; final: boolean }
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
    .map((u) => u.text)
    .filter(Boolean)
    .join(' ')
  if (!body) return { value: before + after, caret: before.length }
  const lead = before && !/\s$/.test(before) ? ' ' : ''
  const trail = after && !/^[\s.,;:!?)]/.test(after) ? ' ' : ''
  const region = lead + body + trail
  return { value: before + region + after, caret: before.length + lead.length + body.length }
}

/** Pure reducer over the five `Dictation*` events; ids not ours only move `elsewhere`. */
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
      while (utts.length < e.utterance) utts.push({ text: '', final: true })
      utts[e.utterance] = { text: e.text, final: e.final }
      return { ...s, active: { ...s.active, utts } }
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
const finishers = new Set<(r: Splice) => void>()

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
  return useSyncExternalStore(subscribe, snapshot)
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

function handle(e: VoiceEvent): void {
  const prev = state
  const next = applyEvent(prev, e)
  set(next)
  if (e.type !== 'DictationFinished' || !prev.active || next.active) return
  const a = prev.active
  const r = discarding ? { value: a.before + a.after, caret: a.before.length } : splice(a.before, a.after, a.utts)
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
    on('DictationFinished', handle),
  ]
  return () => {
    for (const off of offs) off()
  }
}

/** Fired once when our dictation ends (daemon `Finished` after stop, discard, error or max length) with the value to put in the composer. */
export function onFinish(cb: (r: Splice) => void): () => void {
  finishers.add(cb)
  return () => {
    finishers.delete(cb)
  }
}

/** Anchor the caret (`before`/`after`) and ask the daemon for the mic; a refusal toasts and returns to idle. */
export async function start(api: Api, before: string, after: string): Promise<void> {
  if (state.active) return
  set({
    ...state,
    phase: 'starting',
    active: { id: '', before, after, utts: [], levels: [], startedAt: Date.now() },
  })
  try {
    const id = await api.dictation_start()
    // `state` was narrowed to `active: null` above and TS keeps that across calls; read it fresh.
    const a = snapshot().active
    if (a?.id === '') set({ ...state, active: { ...a, id } })
  } catch (e) {
    discarding = false
    set({ ...state, phase: 'idle', active: null })
    toast.error(e instanceof Error ? e.message : String(e))
    void refreshStatus(api).catch(() => undefined)
  }
}

/** Stop and keep the region; the `DictationFinished` event completes the transition. */
export async function stop(api: Api): Promise<void> {
  const a = state.active
  if (!a?.id || state.phase === 'stopping') return
  set({ ...state, phase: 'stopping' })
  await api.dictation_stop(a.id).catch((e: unknown) => {
    toast.error(e instanceof Error ? e.message : String(e))
  })
}

/** Esc: stop and restore `before + after` exactly. */
export function discard(api: Api): Promise<void> {
  if (state.active?.id) discarding = true
  return stop(api)
}

/** `dictation_status` → model state and whether another frontend is dictating. */
export async function refreshStatus(api: Api): Promise<void> {
  const st = await api.dictation_status()
  set({ ...state, model: st.model, elsewhere: st.active !== null && st.active !== state.active?.id })
}
