import '../settings/test-dom'
import { act, createElement as h, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { openWhiteboardTile } from '@/components/tiles/open'
import type * as Open from '@/components/tiles/open'
import { TileHost } from '@/components/tiles/host'
import { SidebarProvider } from '@/components/ui/sidebar'
import type { Entry } from '@/store/sessions'
import { MessageTool } from './MessageTool'
import { isDeliverable } from './ToolRunCard'
import { whiteboardCall } from './WhiteboardViewCard'

// The whiteboard opener is stubbed, as in tile-arrival.test.ts: the Trellis test workspace registers no
// `tile:whiteboard` view type. `null` = no tile could open there.
vi.mock('@/components/tiles/open', async (actual) => ({
  ...(await actual<typeof Open>()),
  openWhiteboardTile: vi.fn((host: string): string | null => (host === 's1' ? 'whiteboard-tile' : null)),
}))
const opened = vi.mocked(openWhiteboardTile)

const nav = vi.hoisted(() => ({ navigate: vi.fn() }))
vi.mock('@tanstack/react-router', async (original) => ({ ...(await original<object>()), useNavigate: () => nav.navigate }))

const WB = '01jabcdefghjkmnpqrstvwxyz0'
type Tool = Extract<Entry, { kind: 'tool' }>
const tool = (over: Partial<Tool>): Tool => ({ id: 'e1', kind: 'tool', callId: 'c1', name: 'show_whiteboard', args: { whiteboard: WB }, pending: false, child: false, ...over })
const result = (output: string, isError = false) => ({ output, isError, files: [] })
const SHOWN = `opened whiteboard "Checkout \\"ideas\\""\ntheuth://whiteboards/${WB}\n3 cards`
const CREATED = `created whiteboard "Checkout ideas" ${WB} — show it to the user with show_whiteboard("${WB}")\ntheuth://whiteboards/${WB}\n0 cards`

beforeEach(() => {
  opened.mockClear()
  nav.navigate.mockClear()
})
afterEach(() => {
  document.body.innerHTML = ''
})

test('both receipts name the title, the whiteboard and its card count; older receipts have no count', () => {
  expect(whiteboardCall(tool({ result: result(SHOWN) }))).toEqual({ title: 'Checkout "ideas"', doc: WB, cards: 3 })
  expect(whiteboardCall(tool({ name: 'wb_create', result: result(CREATED) }))).toEqual({ title: 'Checkout ideas', doc: WB, cards: 0 })
  const old = `created whiteboard "Plan" ${WB} — show it with show_whiteboard("${WB}")`
  expect(whiteboardCall(tool({ name: 'wb_create', result: result(old) }))).toEqual({ title: 'Plan', doc: WB })
  expect(whiteboardCall(tool({ pending: true, args: { title: 'Plan' } }))).toEqual({ title: 'Plan', doc: null })
  expect(whiteboardCall(tool({ result: result('show_whiteboard: d1 is a note; use show_note', true) })).doc).toBeNull()
})

test('a whiteboard card is never folded into a run of tools, like every show', () => {
  expect(isDeliverable(tool({}))).toBe(true)
  expect(isDeliverable(tool({ name: 'wb_create' }))).toBe(true)
})

const mount = async (node: ReactNode, host: string | null) => {
  const el = document.body.appendChild(document.createElement('div'))
  const root = createRoot(el)
  await act(async () => root.render(h(SidebarProvider, null, h(TileHost.Provider, { value: host }, node))))
  return { el, root }
}
const openButton = (el: HTMLElement) => [...el.querySelectorAll('button')].find((b) => /^(Open|Reopen|Focus)$/.test(b.textContent ?? ''))!

test('the transcript renders a created whiteboard as its card; Open puts the whiteboard tile beside the chat', async () => {
  const { el, root } = await mount(h(MessageTool, { tool: tool({ name: 'wb_create', callId: 'c1.1', child: true, args: { title: 'Checkout ideas' }, result: result(CREATED) }), session: 's1' }), 's1')
  const card = el.querySelector<HTMLElement>(`[data-whiteboard-card="${WB}"]`)!
  expect(card.textContent).toContain('Checkout ideas')
  expect(card.textContent).toContain('New whiteboard · empty')
  await act(async () => openButton(el).click())
  expect(opened.mock.calls).toEqual([['s1', { doc: WB, title: 'Checkout ideas' }]])
  expect(nav.navigate).not.toHaveBeenCalled()
  await act(async () => root.unmount())
})

test('with no tile host (the iPad, a phone) Open goes to the whiteboard page, bound to the chat', async () => {
  const { el, root } = await mount(h(MessageTool, { tool: tool({ result: result(SHOWN) }), session: 's1' }), null)
  expect(el.textContent).toContain('Whiteboard · 3 cards')
  await act(async () => openButton(el).click())
  expect(nav.navigate).toHaveBeenCalledWith({ to: '/whiteboards/$id', params: { id: WB }, search: { session: 's1' } })
  expect(opened).not.toHaveBeenCalled()
  await act(async () => root.unmount())
})
