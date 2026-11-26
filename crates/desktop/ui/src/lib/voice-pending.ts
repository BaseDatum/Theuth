// 45 §7.2: a voice note between its release and its `[voice note, N s]` row. The note goes up whole
// (`put_attachment`, one RPC: no byte progress), then `transcribe` runs on the engine (seconds per minute of audio),
// then the composer sends the line. Each phase is its own RPC, so the client knows the phase without an engine event.
// Until the row lands the note is a pending bubble — at the transcript's end, or over the box of a composer with no
// transcript — and a failure keeps the recording for Retry: only Discard or a landed send drops it.

import { create } from 'zustand'

export type NotePhase = 'uploading' | 'transcribing' | 'failed'

export type PendingNote = {
  id: string
  /** The recording; kept until the note lands or is discarded. */
  blob: Blob
  /** Recorded length. */
  ms: number
  /** The session whose transcript shows the bubble; `null`: the composer shows it over its box. */
  session: string | null
  phase: NotePhase
  /** When the recording was handed over: the bubbles' order. */
  created: number
  /** When the current phase began (ms since epoch): the elapsed time on the bubble. */
  since: number
  /** One per run of the pipeline: the first, then every Retry. */
  attempt: number
  /** The attempt a composer has taken to run (`claim`): two panes on one draft run it once. */
  claimed: number
  error?: string
}

export type NoteEvent =
  | { type: 'uploaded'; at: number }
  /** The `[voice note …]` line went to `submit`: from here the sessions store shows the row. */
  | { type: 'landed' }
  | { type: 'failed'; error: string; at: number }
  | { type: 'retry'; at: number }

/** Pure: the note after `e`; `null` once it landed. An event out of phase changes nothing. */
export function stepNote(n: PendingNote, e: NoteEvent): PendingNote | null {
  switch (e.type) {
    case 'uploaded':
      return n.phase === 'uploading' ? { ...n, phase: 'transcribing', since: e.at } : n
    case 'landed':
      return n.phase === 'transcribing' ? null : n
    case 'failed':
      return n.phase === 'failed' ? n : { ...n, phase: 'failed', since: e.at, error: e.error }
    case 'retry':
      return n.phase === 'failed' ? { ...n, phase: 'uploading', since: e.at, attempt: n.attempt + 1, error: undefined } : n
  }
}

/** `0:07`, `2:13`: the elapsed time on the bubble. */
export const clock = (ms: number): string => {
  const s = Math.max(0, Math.floor(ms / 1000))
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`
}

type Store = {
  /** By the composer's draft key: one note at a time per composer. */
  by: Record<string, PendingNote>
  /** A finished recording; `false` (nothing changes) while one is already pending there. */
  start(key: string, note: { blob: Blob; ms: number; session: string | null }, at?: number): boolean
  /** Take `attempt` to run; `true` once per attempt. */
  claim(key: string, id: string, attempt: number): boolean
  /** `e` for note `id`'s run `attempt`; a stale run (the note discarded or retried since) changes nothing. */
  apply(key: string, id: string, attempt: number, e: NoteEvent): void
  retry(key: string, at?: number): void
  /** Drop a failed note (and its recording). */
  discard(key: string): void
}

const put = (by: Record<string, PendingNote>, key: string, n: PendingNote | null): Record<string, PendingNote> => {
  const next = { ...by }
  if (n) next[key] = n
  else delete next[key]
  return next
}

export const usePendingNotes = create<Store>()((set, get) => ({
  by: {},
  start(key, { blob, ms, session }, at = Date.now()) {
    if (get().by[key]) return false
    const note: PendingNote = { id: crypto.randomUUID(), blob, ms, session, phase: 'uploading', created: at, since: at, attempt: 1, claimed: 0 }
    set((s) => ({ by: put(s.by, key, note) }))
    return true
  },
  claim(key, id, attempt) {
    const n = get().by[key]
    if (!n || n.id !== id || n.attempt !== attempt || n.claimed >= attempt || n.phase === 'failed') return false
    set((s) => ({ by: put(s.by, key, { ...n, claimed: attempt }) }))
    return true
  },
  apply(key, id, attempt, e) {
    const n = get().by[key]
    if (!n || n.id !== id || n.attempt !== attempt) return
    const next = stepNote(n, e)
    if (next !== n) set((s) => ({ by: put(s.by, key, next) }))
  },
  retry(key, at = Date.now()) {
    const n = get().by[key]
    if (!n) return
    const next = stepNote(n, { type: 'retry', at })
    if (next !== n) set((s) => ({ by: put(s.by, key, next) }))
  },
  discard(key) {
    if (get().by[key]?.phase === 'failed') set((s) => ({ by: put(s.by, key, null) }))
  },
}))

/** The notes a session's transcript shows, oldest first; select `by` and memoise: this is a new array every call. */
export const notesFor = (by: Record<string, PendingNote>, session: string): [string, PendingNote][] =>
  Object.entries(by)
    .filter(([, n]) => n.session === session)
    .sort(([, a], [, b]) => a.created - b.created)
