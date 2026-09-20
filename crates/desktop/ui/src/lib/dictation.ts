// 54 §5: live dictation into the composer. The daemon owns the model and, for the desktop, the mic;
// a relay-linked client streams its own mic instead (§4.5). This module keeps the caret anchor
// (`before`/`after`), the utterances the daemon sends, the lifecycle, and the browser capture graph,
// and hands the composer the spliced value when a dictation ends. Insert-only: `before` and `after`
// are never touched.

import { type RefObject, useEffect, useRef, useState, useSyncExternalStore } from 'react'
import { toast } from 'sonner'
import { Batcher } from '@/lib/pcm'
import * as speech from '@/lib/speech'
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

/** The browser-side mic graph of a remote-source dictation (54 §4.5); `null` while the daemon owns the mic. */
type Capture = { stream: MediaStream; ctx: AudioContext; nodes: AudioNode[]; batcher: Batcher | null }
let capture: Capture | null = null
/** Contexts whose worklet already has `theuth-tap`; a second `registerProcessor` of the same name rejects. */
const tapped = new WeakSet<AudioContext>()

/** One `AudioWorkletProcessor` that posts each 128-frame render quantum of its mono input to the main thread. */
const WORKLET_SRC = `registerProcessor('theuth-tap', class extends AudioWorkletProcessor {
  process(inputs) { const ch = inputs[0][0]; if (ch) this.port.postMessage(ch.slice()); return true }
})`

/** Release the mic and the tap nodes; idempotent. The context is speech's and stays open. */
function teardownCapture(): void {
  const c = capture
  if (!c) return
  capture = null
  for (const t of c.stream.getTracks()) t.stop()
  for (const n of c.nodes) n.disconnect()
}

/**
 * Open the mic inside the click gesture (the permission prompt needs it) on speech's one device-rate
 * context; the daemon resamples (ponytail: ~1 Mbit/s of PCM at 48 kHz — decimate here if the relay minds).
 */
async function openCapture(): Promise<Capture> {
  const ctx = speech.unlock()
  const stream = await navigator.mediaDevices.getUserMedia({
    audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true },
  })
  const c: Capture = { stream, ctx, nodes: [], batcher: null }
  capture = c
  if (ctx.sampleRate % 50 !== 0) throw new Error('unsupported microphone rate')
  return c
}

/** Mic → tap → 100 ms i16 batches → `dictation_audio`; never awaits inside the audio path, a failed push is dropped. */
async function wire(api: Api, id: string, c: Capture): Promise<void> {
  const { ctx } = c
  const batcher = new Batcher(ctx.sampleRate / 10, (b64) => void api.dictation_audio(id, b64).catch(() => undefined))
  // A tap has to sit on a path to `destination` or the graph never renders it; zero gain keeps it silent.
  const mute = ctx.createGain()
  mute.gain.value = 0
  mute.connect(ctx.destination)
  let tap: AudioNode
  if (ctx.audioWorklet) {
    if (!tapped.has(ctx)) {
      // `blob:` needs `script-src blob:` in the Tauri CSP (tauri.conf.json).
      const url = URL.createObjectURL(new Blob([WORKLET_SRC], { type: 'text/javascript' }))
      try {
        await ctx.audioWorklet.addModule(url)
      } finally {
        URL.revokeObjectURL(url)
      }
      tapped.add(ctx)
    }
    // Torn down while the module loaded (Esc, a fast `DictationFinished`).
    if (capture !== c) return
    const node = new AudioWorkletNode(ctx, 'theuth-tap')
    node.port.onmessage = (e: MessageEvent<Float32Array>) => batcher.push(e.data)
    tap = node
  } else {
    // ponytail: `createScriptProcessor` is deprecated but the only tap where `audioWorklet` is missing.
    const node = ctx.createScriptProcessor(2048, 1, 1)
    node.onaudioprocess = (e) => batcher.push(e.inputBuffer.getChannelData(0))
    tap = node
  }
  const src = ctx.createMediaStreamSource(c.stream)
  src.connect(tap).connect(mute)
  c.nodes = [src, tap, mute]
  c.batcher = batcher
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

/**
 * One text box with a mic button (StartForm's context, AskCard's answer): the store is global with one
 * `active`, so the box takes an `owner` flag exactly as `Composer.tsx` does and splices the finished text
 * at its caret. `shown` is the live spliced value while dictating; render it read-only then.
 */
export function useBox(
  api: Api,
  value: string,
  onChange: (v: string) => void,
  ref: RefObject<HTMLInputElement | HTMLTextAreaElement | null>,
  mic: boolean,
): { dict: State; dictating: boolean; reason: string | null; toggle: () => void; shown: string } {
  const dict = useDictation()
  const [owner, setOwner] = useState(false)
  const ownerRef = useRef(owner)
  ownerRef.current = owner
  const dictating = owner && dict.active !== null
  const why = reason(dict) ?? (dict.active !== null && !owner ? 'Dictating in another box' : null)
  useEffect(
    () =>
      onFinish(({ value }) => {
        if (ownerRef.current) onChange(value)
      }),
    [onChange],
  )
  useEffect(() => {
    if (dict.phase === 'idle') setOwner(false)
  }, [dict.phase])
  const toggle = () => {
    if (dictating) {
      if (dict.phase === 'dictating') void stop(api)
      return
    }
    if (why) {
      toast(why)
      return
    }
    if (dict.phase !== 'idle') return
    const at = ref.current?.selectionStart ?? value.length
    setOwner(true)
    void start(api, value.slice(0, at), value.slice(at), mic)
  }
  const shown = dictating && dict.active ? splice(dict.active.before, dict.active.after, dict.active.utts).value : value
  return { dict, dictating, reason: why, toggle, shown }
}

function handle(e: VoiceEvent): void {
  const prev = state
  const next = applyEvent(prev, e)
  set(next)
  if (e.type !== 'DictationFinished' || !prev.active || next.active) return
  teardownCapture()
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

/**
 * Anchor the caret (`before`/`after`) and start dictating; a refusal toasts and returns to idle. `mic`:
 * stream this browser's microphone to the daemon (a relay peer; 54 §4.5) instead of using the harness mic.
 */
export async function start(api: Api, before: string, after: string, mic: boolean): Promise<void> {
  if (state.active) return
  set({
    ...state,
    phase: 'starting',
    active: { id: '', before, after, utts: [], levels: [], startedAt: Date.now() },
  })
  let id: string | null = null
  try {
    const c = mic ? await openCapture() : null
    id = await api.dictation_start(c ? c.ctx.sampleRate : null)
    // `state` was narrowed to `active: null` above and TS keeps that across calls; read it fresh.
    const a = snapshot().active
    if (a?.id === '') set({ ...state, active: { ...a, id } })
    if (c) await wire(api, id, c)
  } catch (e) {
    teardownCapture()
    // The daemon already holds the slot: hand it back; its `Finished` finds nothing active here and is ignored.
    if (id !== null) void api.dictation_stop(id).catch(() => undefined)
    discarding = false
    set({ ...state, phase: 'idle', active: null })
    toast.error(e instanceof Error ? e.message : String(e))
    void refreshStatus(api).catch(() => undefined)
  }
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
