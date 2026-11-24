import '../settings/test-dom'
import { act, createElement as h, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { TileHost } from '@/components/tiles/host'
import { SidebarProvider } from '@/components/ui/sidebar'
import { tileLeaf, usePanes } from '@/lib/panes'
import { tileIds, tileProps, tiling, untile } from '@/lib/panes.testing'
import { tileId } from '@/lib/tiles'
import type { Entry } from '@/store/sessions'
import { NoteViewCard, noteCall } from './NoteViewCard'

const nav = vi.hoisted(() => ({ navigate: vi.fn() }))
vi.mock('@tanstack/react-router', async (original) => ({ ...(await original<object>()), useNavigate: () => nav.navigate }))
vi.mock('@/lib/popout', async (original) => ({ ...(await original<object>()), canPopout: () => true, openPopout: vi.fn(async () => {}) }))

const DOC = '01jabcdefghjkmnpqrstvwxyz0'
type Tool = Extract<Entry, { kind: 'tool' }>
const tool = (over: Partial<Tool>): Tool => ({ id: 'e1', kind: 'tool', callId: 'c1', name: 'show_note', args: { doc: 'knowledge:a/b' }, pending: false, child: false, ...over })
const result = (output: string, isError = false) => ({ output, isError, files: [] })
const RECEIPT = `note view opened · Release plan · tile note:${DOC}\ntheuth://notes/${DOC}#b7\nIt updates live as you edit it with doc_insert/doc_replace.`

beforeEach(() => {
  tiling(['s1'])
  nav.navigate.mockClear()
})
afterEach(() => {
  untile()
  document.body.innerHTML = ''
})

test('the receipt names the title and the document; the link line carries the block when there is one', () => {
  expect(noteCall(tool({ result: result(RECEIPT) }))).toEqual({ title: 'Release plan', doc: DOC, block: 'b7' })
  const plain = `note view opened · Release plan · tile note:${DOC}\ntheuth://notes/${DOC}`
  expect(noteCall(tool({ result: result(plain) }))).toEqual({ title: 'Release plan', doc: DOC })
})

test('pending, failed or malformed calls have no document; the arguments name them', () => {
  expect(noteCall(tool({ pending: true }))).toEqual({ title: 'knowledge:a/b', doc: null })
  expect(noteCall(tool({ args: { doc: DOC, title: 'Plan' } }))).toEqual({ title: 'Plan', doc: null })
  expect(noteCall(tool({ result: result('show_note: notebook documents do not open in a note tile', true) })).doc).toBeNull()
  // A DocId is exactly 26 lowercase base32 characters.
  expect(noteCall(tool({ result: result('note view opened · x · tile note:SHORT') })).doc).toBeNull()
})

const mount = async (node: ReactNode, host: string | null) => {
  const el = document.body.appendChild(document.createElement('div'))
  const root = createRoot(el)
  await act(async () => root.render(h(SidebarProvider, null, h(TileHost.Provider, { value: host }, node))))
  return { el, root }
}
const openButton = (el: HTMLElement) => [...el.querySelectorAll('button')].find((b) => b.textContent === 'Open')!

test('Open puts the note tile beside the chat where the layout tiles', async () => {
  const { el, root } = await mount(h(NoteViewCard, { tool: tool({ result: result(RECEIPT) }) }), 's1')
  await act(async () => openButton(el).click())
  const id = tileId('note', 's1', DOC)
  expect(usePanes.getState().leaves).toEqual(['s1', tileLeaf(id)])
  expect(tileProps(id)).toEqual({ doc: DOC, title: 'Release plan', block: 'b7' })
  expect(nav.navigate).not.toHaveBeenCalled()
  await act(async () => root.unmount())
})

test('with no tile host (a phone) Open goes to the note page at the block', async () => {
  const { el, root } = await mount(h(NoteViewCard, { tool: tool({ result: result(RECEIPT) }) }), null)
  await act(async () => openButton(el).click())
  expect(nav.navigate).toHaveBeenCalledWith({ to: '/notes/$id', params: { id: DOC }, hash: 'b7' })
  expect(tileIds()).toEqual([])
  await act(async () => root.unmount())
})
