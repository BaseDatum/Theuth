import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { popoutOf } from '@/components/chat/AttachChip'
import { type AttachRef, DRAG_TYPE, addRefs, droppedRef } from '@/lib/attach'
import { AttachmentChips, AttachmentCount } from './Attachments'

vi.mock('@tanstack/react-router', () => ({ useNavigate: () => vi.fn() }))

const note: AttachRef = { kind: 'note', id: 'n1', title: 'Groceries', meta: 'updated Sep 24' }
const chat: AttachRef = { kind: 'session', id: 's1', title: 'Fix relay', meta: '' }
const wiki: AttachRef = { kind: 'knowledge', id: 'facts/x@3f2a', title: 'X', meta: '' }

test('task attachment chips: title and kind per ref, a remove × only when editable', () => {
  const html = renderToStaticMarkup(createElement(AttachmentChips, { items: [note, chat] }))
  expect(html).toContain('Groceries')
  expect(html).toContain('Fix relay')
  expect(html).toContain('aria-label="Note: Groceries"')
  expect(html).toContain('aria-label="Chat: Fix relay"')
  expect(html).not.toContain('Remove ')
  const editable = renderToStaticMarkup(createElement(AttachmentChips, { items: [note], onRemove: () => {} }))
  expect(editable).toContain('aria-label="Remove Groceries"')
})

test('the board card paperclip shows the count and nothing for none', () => {
  expect(renderToStaticMarkup(createElement(AttachmentCount, { items: [] }))).toBe('')
  const two = renderToStaticMarkup(createElement(AttachmentCount, { items: [note, chat] }))
  expect(two).toContain('aria-label="2 attachments"')
  expect(two).toMatch(/>\s*(<!-- -->\s*)?2<\/span>/)
})

test('picker and drop wiring: refs merge by kind + id, order kept; a foreign drop is ignored', () => {
  expect(addRefs([note], [{ ...note, title: 'renamed' }, chat])).toEqual([note, chat])
  expect(addRefs([], [chat, chat])).toEqual([chat])
  const dt = (data: Record<string, string>) => ({ getData: (t: string) => data[t] ?? '' }) as unknown as DataTransfer
  expect(droppedRef(dt({ [DRAG_TYPE]: JSON.stringify(note) }))).toEqual(note)
  expect(droppedRef(dt({ [DRAG_TYPE]: JSON.stringify({ kind: 'note', id: 'n2' }) }))).toEqual({ kind: 'note', id: 'n2', title: 'n2', meta: '' })
  expect(droppedRef(dt({ [DRAG_TYPE]: JSON.stringify({ kind: 'todo', id: 'x' }) }))).toBeNull()
  expect(droppedRef(dt({ [DRAG_TYPE]: '{not json' }))).toBeNull()
  expect(droppedRef(dt({ 'text/plain': 'hello' }))).toBeNull()
})

test('open-or-popout: every kind with a popped route maps to it; knowledge has none', () => {
  expect(popoutOf(note)).toEqual({ kind: 'note', doc: 'n1' })
  expect(popoutOf(chat)).toEqual({ kind: 'chat', session: 's1' })
  expect(popoutOf({ kind: 'artifact', id: 'toolu_1', title: 'r', meta: '' })).toEqual({ kind: 'artifact', callId: 'toolu_1' })
  expect(popoutOf({ kind: 'meeting', id: 'm1', title: 'w', meta: '' })).toEqual({ kind: 'meeting', meeting: 'm1' })
  expect(popoutOf(wiki)).toBeNull()
})
