import { beforeEach, expect, test, vi } from 'vitest'
import type { Api } from '@/rpc/bindings/client'
import { EMPTY, NO_PICK, PLAIN_OPTIONS, load, openDraftSession, useDrafts } from './drafts'
import { attach, attachRef } from './pastes'

// No DOM in the test runner: the smallest `Storage` the store needs (the module's own `load()` ran without one and
// read as empty, which is the corrupt/absent-storage path).
const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => mem.get(k) ?? null,
  setItem: (k: string, v: string) => void mem.set(k, v),
  clear: () => mem.clear(),
})

const ref = { kind: 'note' as const, id: 'n1', title: 'Plan', meta: '' }
const held = attachRef(ref, attach('/tmp/a.png', 4, 4, EMPTY.pastes))

beforeEach(() => {
  localStorage.clear()
  useDrafts.setState({ by: {}, edits: {}, pick: NO_PICK })
})

test('draft → edit → cancel leaves the composer draft intact (text, pastes, attachments)', () => {
  const s = useDrafts.getState()
  s.set('c1', { text: 'half a thought ⟦paste 1⟧', pastes: held })
  s.beginEdit('c1', { turn: 3, text: 'sent words', refs: [ref] })
  s.updateEdit('c1', { text: 'sent words, changed', refs: [] })
  expect(useDrafts.getState().edits.c1).toEqual({ turn: 3, text: 'sent words, changed', refs: [] })
  s.endEdit('c1')
  expect(useDrafts.getState().edits.c1).toBeUndefined()
  expect(useDrafts.getState().by.c1).toEqual({ text: 'half a thought ⟦paste 1⟧', pastes: held })
})

test('one draft per session and one for the new chat', () => {
  const s = useDrafts.getState()
  s.setText('', 'new chat words')
  s.setText('c1', 'chat one')
  s.setPastes('c1', (p) => attachRef(ref, p))
  expect(useDrafts.getState().by['']).toEqual({ text: 'new chat words', pastes: EMPTY.pastes })
  expect(useDrafts.getState().by.c1).toEqual({ text: 'chat one', pastes: attachRef(ref, EMPTY.pastes) })
})

test('reload restores drafts and an open edit; an empty draft is not stored', () => {
  const s = useDrafts.getState()
  s.set('c1', { text: 'kept', pastes: held })
  s.setText('c2', '')
  s.beginEdit('c1', { turn: 2, text: 'editing', refs: [ref] })
  const back = load()
  expect(back.by.c1).toEqual({ text: 'kept', pastes: held })
  expect(back.by.c1?.pastes).toBeInstanceOf(Map)
  expect(back.by.c2).toBeUndefined()
  expect(back.edits).toEqual({ c1: { turn: 2, text: 'editing', refs: [ref] } })
})

test('a send keeps the text until acked; the ack drops exactly what was sent', () => {
  const s = useDrafts.getState()
  s.set('c1', { text: 'long thought ⟦paste 1⟧', pastes: held })
  s.sending('c1', 'id-1')
  // In flight: still in the box, flagged, and persisted so a reload mid-send shows the banner.
  expect(useDrafts.getState().by.c1).toEqual({ text: 'long thought ⟦paste 1⟧', pastes: held, unsent: 'id-1' })
  expect(load().by.c1?.unsent).toBe('id-1')
  // Typed during the flight: only the sent text leaves.
  s.setText('c1', 'long thought ⟦paste 1⟧ and more')
  s.acked('c1', 'long thought ⟦paste 1⟧')
  expect(useDrafts.getState().by.c1).toEqual({ text: ' and more', pastes: EMPTY.pastes })
  s.sending('c1', 'id-2')
  s.acked('c1', ' and more')
  expect(useDrafts.getState().by.c1).toEqual({ text: '', pastes: EMPTY.pastes })
  expect(load().by.c1).toBeUndefined()
})

test('a failed send leaves the flagged draft through edits; emptying the box drops the flag', () => {
  const s = useDrafts.getState()
  s.set('c1', { text: 'kept', pastes: held })
  s.sending('c1', 'id-1')
  // Failure changes nothing in the store: the composer reads `unsent` for the banner and the retry id.
  expect(useDrafts.getState().by.c1).toEqual({ text: 'kept', pastes: held, unsent: 'id-1' })
  s.setText('c1', 'kept, edited')
  expect(useDrafts.getState().by.c1?.unsent).toBe('id-1')
  s.setText('c1', '')
  // Chips still held: there is still something to retry.
  expect(useDrafts.getState().by.c1).toEqual({ text: '', pastes: held, unsent: 'id-1' })
  s.set('c1', EMPTY)
  expect(useDrafts.getState().by.c1).toEqual({ text: '', pastes: EMPTY.pastes })
})

test('a corrupt store reads as empty', () => {
  localStorage.setItem('composer-drafts', '{not json')
  expect(load()).toEqual({ by: {}, edits: {}, pick: NO_PICK })
})

// 148 R1: the one seam every draft-born session goes through (the composer's send, Talk, `/<skill>`, a parked skill).
const opened = { session: 's-new', retention: 'durable' }
const apiFor = () => {
  const create_session = vi.fn(async () => opened)
  const new_session = vi.fn(async () => 's-plain')
  return { api: { create_session, new_session } as unknown as Api, create_session, new_session }
}

test('148 R2: the draft pick lives with the `\'\'` draft text — persisted in the same blob, back after a remount/reload', () => {
  const s = useDrafts.getState()
  s.setText('', 'half a thought')
  s.setPick({ model: 'openai/gpt-5.4' })
  s.setPick({ effort: 'high' })
  expect(useDrafts.getState().pick).toEqual({ model: 'openai/gpt-5.4', effort: 'high' })
  // A route round-trip remounts NewChat; a relaunch or another window's save re-reads the store: both keep the pick.
  useDrafts.setState(load())
  expect(useDrafts.getState().by['']).toEqual({ text: 'half a thought', pastes: EMPTY.pastes })
  expect(useDrafts.getState().pick).toEqual({ model: 'openai/gpt-5.4', effort: 'high' })
  // A blob saved before the pick existed reads as no pick.
  localStorage.setItem('composer-drafts', JSON.stringify({ drafts: {}, edits: {} }))
  expect(load().pick).toEqual(NO_PICK)
})

test('148 R1: a draft model/effort pick opens through create_session with it (never new_session) and is spent', async () => {
  const { api, create_session, new_session } = apiFor()
  useDrafts.getState().setPick({ model: 'openai/gpt-5.4', effort: 'high' })
  const r = await openDraftSession(api)
  expect(create_session).toHaveBeenCalledWith({ ...PLAIN_OPTIONS, model: 'openai/gpt-5.4', effort: 'high' })
  expect(new_session).not.toHaveBeenCalled()
  expect(r).toEqual({ id: 's-new', open: opened, pick: { model: 'openai/gpt-5.4', effort: 'high' } })
  // Spent in the store and in storage: the next draft starts on the engine default.
  expect(useDrafts.getState().pick).toEqual(NO_PICK)
  expect(load().pick).toEqual(NO_PICK)
  // Effort alone is enough: `new_session()` cannot carry it.
  useDrafts.getState().setPick({ effort: 'low' })
  await openDraftSession(api)
  expect(create_session).toHaveBeenLastCalledWith({ ...PLAIN_OPTIONS, effort: 'low' })
  expect(useDrafts.getState().pick.effort).toBeNull()
})

test('148 R1: no pick and no switch → new_session(); a switch (the composer\'s options) → create_session with them', async () => {
  const { api, create_session, new_session } = apiFor()
  expect(await openDraftSession(api)).toEqual({ id: 's-plain', open: null, pick: NO_PICK })
  expect(create_session).not.toHaveBeenCalled()
  const options = { ...PLAIN_OPTIONS, project: 'lab', model: 'openai/gpt-5.4' }
  useDrafts.getState().setPick({ model: 'openai/gpt-5.4' })
  expect((await openDraftSession(api, options)).id).toBe('s-new')
  expect(create_session).toHaveBeenCalledWith(options)
  expect(new_session).toHaveBeenCalledTimes(1)
  expect(useDrafts.getState().pick.model).toBeNull()
})
