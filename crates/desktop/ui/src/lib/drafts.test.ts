import { beforeEach, expect, test, vi } from 'vitest'
import { EMPTY, load, useDrafts } from './drafts'
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
  useDrafts.setState({ by: {}, edits: {} })
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

test('a corrupt store reads as empty', () => {
  localStorage.setItem('composer-drafts', '{not json')
  expect(load()).toEqual({ by: {}, edits: {} })
})
