import { expect, test } from 'vitest'
import type { Artifact } from '@/rpc/bindings/Artifact'
import type { AttachRef } from './attach'
import {
  categoriesFor,
  inThisChat,
  keyAction,
  latestVersions,
  nextTab,
  pushRecents,
  stripAt,
} from './attach-picker'

const art = (call_id: string, session: string, name: string, version: number, created: number): Artifact => ({
  id: 'sha',
  session,
  call_id,
  name,
  title: name,
  kind: 'pdf',
  mime: 'application/pdf',
  bytes: 10n,
  version,
  path: '/p',
  source: null,
  preview: null,
  pages: null,
  origin: null,
  created_at: BigInt(created),
})

test('latest version per (session, name), newest first', () => {
  const rows = [art('a1', 's1', 'r.pdf', 1, 100), art('a2', 's1', 'r.pdf', 2, 200), art('b1', 's2', 'r.pdf', 1, 300), art('c1', 's1', 'x.csv', 1, 50)]
  expect(latestVersions(rows).map((a) => a.call_id)).toEqual(['b1', 'a2', 'c1'])
})

test('in this chat: transcript markers newest first, then session artifacts, deduped and capped', () => {
  const entries = [
    { kind: 'user', text: '[attached note n1 "Groceries"]\nadd milk' },
    { kind: 'assistant', text: '[attached note n9 "not a user row"]' },
    { kind: 'user', text: '[attached artifact a2 "r.pdf" · pdf]\n[attached meeting m1 "Weekly"]\nfold' },
    { kind: 'user', text: '[attached note n1 "Groceries (renamed)"]\nagain' },
  ]
  const refs = inThisChat([art('a1', 's1', 'r.pdf', 1, 100), art('a2', 's1', 'r.pdf', 2, 200)], entries)
  expect(refs.map((r) => `${r.kind}:${r.id}`)).toEqual(['note:n1', 'meeting:m1', 'artifact:a2'])
  // The newest mention's title wins.
  expect(refs[0]?.title).toBe('Groceries (renamed)')
  expect(inThisChat([], entries, 1)).toHaveLength(1)
})

test('recents: sent first, deduped, capped at 20', () => {
  const r = (id: string): AttachRef => ({ kind: 'note', id, title: id, meta: '' })
  const old = Array.from({ length: 20 }, (_, i) => r(`o${i}`))
  const next = pushRecents(old, [r('new'), r('o3')])
  expect(next).toHaveLength(20)
  expect(next.slice(0, 2).map((x) => x.id)).toEqual(['new', 'o3'])
  expect(next.filter((x) => x.id === 'o3')).toHaveLength(1)
  expect(next.at(-1)?.id).toBe('o18')
})

test('tabs map to search categories and cycle', () => {
  expect(categoriesFor('all')).toEqual(['notes', 'knowledge', 'knowledge_history', 'artifacts', 'sessions', 'meetings'])
  expect(categoriesFor('knowledge')).toEqual(['knowledge', 'knowledge_history'])
  expect(categoriesFor('note')).toEqual(['notes'])
  expect(categoriesFor('meeting')).toEqual(['meetings'])
  expect(nextTab('all', 1)).toBe('note')
  expect(nextTab('all', -1)).toBe('meeting')
  expect(nextTab('meeting', 1)).toBe('all')
})

test('keyboard map (§7.7)', () => {
  const k = (key: string, mods: Partial<{ metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean }> = {}) => ({
    key,
    metaKey: false,
    ctrlKey: false,
    shiftKey: false,
    altKey: false,
    ...mods,
  })
  expect(keyAction(k('Enter'), true)).toEqual({ type: 'attach' })
  expect(keyAction(k('Enter', { metaKey: true }), true)).toEqual({ type: 'toggle' })
  expect(keyAction(k('Enter', { ctrlKey: true }), false)).toEqual({ type: 'toggle' })
  expect(keyAction(k('Tab'), false)).toEqual({ type: 'tab', dir: 1 })
  expect(keyAction(k('Tab', { shiftKey: true }), false)).toEqual({ type: 'tab', dir: -1 })
  expect(keyAction(k('3', { metaKey: true }), false)).toEqual({ type: 'jump', index: 2 })
  expect(keyAction(k('3'), false)).toBeNull()
  expect(keyAction(k('ArrowRight'), true)).toEqual({ type: 'preview', open: true })
  // Typing a query: arrows move the caret, backspace edits.
  expect(keyAction(k('ArrowRight'), false)).toBeNull()
  expect(keyAction(k('Backspace'), true)).toEqual({ type: 'back' })
  expect(keyAction(k('Backspace'), false)).toBeNull()
  expect(keyAction(k('Escape'), false)).toEqual({ type: 'close' })
  expect(keyAction(k('ArrowDown'), true)).toBeNull()
})

test('@word leaves the text as typed until picked; picking removes it and one following space', () => {
  expect(stripAt('hello @gro', 6, 'gro')).toBe('hello ')
  expect(stripAt('@gro please', 0, 'gro')).toBe('please')
  expect(stripAt('@', 0, '')).toBe('')
})
