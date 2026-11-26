import { createElement as h } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { beforeEach, expect, test } from 'vitest'
import { PendingNoteBubble, phaseLabel } from '@/components/chat/PendingNote'
import { clock, notesFor, stepNote, usePendingNotes } from './voice-pending'

const blob = new Blob(['opus'], { type: 'audio/webm' })
const K = 'sess-1'
const note = () => usePendingNotes.getState().by[K]

beforeEach(() => usePendingNotes.setState({ by: {} }))

test('45 §7.2: a note shows from its release: uploading → transcribing → landed (the row takes over)', () => {
  const st = usePendingNotes.getState()
  expect(st.start(K, { blob, ms: 123_000, session: K }, 1000)).toBe(true)
  const { id } = note()!
  expect(note()).toMatchObject({ phase: 'uploading', since: 1000, ms: 123_000, attempt: 1 })
  expect(phaseLabel(note()!, 5000)).toBe('Uploading…')
  st.apply(K, id, 1, { type: 'uploaded', at: 4000 })
  expect(note()).toMatchObject({ phase: 'transcribing', since: 4000 })
  expect(phaseLabel(note()!, 16_500)).toBe('Transcribing… 0:12')
  st.apply(K, id, 1, { type: 'landed' })
  expect(note()).toBeUndefined()
})

test('45 §7.2: a failure keeps the recording; Retry runs it again from the upload; Discard drops it', () => {
  const st = usePendingNotes.getState()
  st.start(K, { blob, ms: 9000, session: K }, 0)
  const { id } = note()!
  st.apply(K, id, 1, { type: 'uploaded', at: 1 })
  st.apply(K, id, 1, { type: 'failed', error: 'transcriber not running', at: 2 })
  expect(note()).toMatchObject({ phase: 'failed', error: 'transcriber not running', blob })
  expect(phaseLabel(note()!, 3)).toBe('Not sent — transcriber not running')
  // A failed note holds the composer: a second recording is refused, the first one stays.
  expect(st.start(K, { blob: new Blob(['other']), ms: 1000, session: K })).toBe(false)
  expect(note()!.blob).toBe(blob)

  st.retry(K, 10)
  expect(note()).toMatchObject({ phase: 'uploading', attempt: 2, since: 10, error: undefined, blob })
  // A late event of the first run changes nothing.
  st.apply(K, id, 1, { type: 'failed', error: 'stale', at: 11 })
  expect(note()!.phase).toBe('uploading')
  // Discard only takes a failed note (never one on its way).
  st.discard(K)
  expect(note()).toBeDefined()
  st.apply(K, id, 2, { type: 'failed', error: 'offline', at: 12 })
  st.discard(K)
  expect(note()).toBeUndefined()
})

test('45 §7.2: each attempt is claimed once (two panes on one chat run it once), never while failed', () => {
  const st = usePendingNotes.getState()
  st.start(K, { blob, ms: 2000, session: K })
  const { id } = note()!
  expect(st.claim(K, id, 1)).toBe(true)
  expect(st.claim(K, id, 1)).toBe(false)
  expect(st.claim(K, 'other', 1)).toBe(false)
  st.apply(K, id, 1, { type: 'failed', error: 'x', at: 0 })
  expect(st.claim(K, id, 1)).toBe(false)
  st.retry(K)
  expect(st.claim(K, id, 2)).toBe(true)
})

test('stepNote: an event out of phase changes nothing', () => {
  const n = { id: 'a', blob, ms: 1, session: null, phase: 'uploading', created: 0, since: 0, attempt: 1, claimed: 0 } as const
  expect(stepNote(n, { type: 'landed' })).toBe(n)
  expect(stepNote(n, { type: 'retry', at: 1 })).toBe(n)
  const t = stepNote(n, { type: 'uploaded', at: 1 })!
  expect(stepNote(t, { type: 'uploaded', at: 2 })).toBe(t)
})

test('notesFor: the transcript shows its own session’s notes, oldest first; a draft’s note is the composer’s', () => {
  const st = usePendingNotes.getState()
  st.start('b', { blob, ms: 1, session: 'S' }, 20)
  st.start('a', { blob, ms: 1, session: 'S' }, 10)
  st.start('', { blob, ms: 1, session: null }, 5)
  expect(notesFor(usePendingNotes.getState().by, 'S').map(([k]) => k)).toEqual(['a', 'b'])
})

test('the bubble: length under a mic, the phase as a status; failed is an alert with Retry and Discard', () => {
  const st = usePendingNotes.getState()
  st.start(K, { blob, ms: 125_400, session: K })
  const html = () => renderToStaticMarkup(h(PendingNoteBubble, { noteKey: K, note: note()! }))
  expect(html()).toContain('126 s')
  expect(html()).toContain('role="status"')
  expect(html()).toContain('Uploading…')
  expect(html()).not.toContain('Retry')
  st.apply(K, note()!.id, 1, { type: 'failed', error: 'offline', at: Date.now() })
  expect(html()).toContain('role="alert"')
  expect(html()).toContain('Not sent — offline')
  expect(html()).toContain('Retry')
  expect(html()).toContain('Discard')
  expect(clock(59_999)).toBe('0:59')
})
