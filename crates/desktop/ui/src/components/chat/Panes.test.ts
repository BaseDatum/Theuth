import '../settings/test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement as h, useEffect } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { TileFrame } from '@/components/tiles/TileFrame'
import { SidebarProvider } from '@/components/ui/sidebar'
import { STORAGE_KEY, tileLeaf, usePanes } from '@/lib/panes'
import { untile } from '@/lib/panes.testing'
import { closeTile, openTile, tileId } from '@/lib/tiles'
import { Panes } from './Panes'

const navigate = vi.hoisted(() => vi.fn())
vi.mock('@tanstack/react-router', async (original) => ({ ...(await original<object>()), useNavigate: () => navigate }))
vi.mock('@/rpc/capabilities', async (original) => ({ ...(await original<object>()), useQuerySupport: () => false }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<object>()), useApi: () => ({}) }))
vi.mock('@/lib/hotkeys', async (original) => ({ ...(await original<object>()), isMacDesktop: () => true }))

const mounts: Record<string, number> = {}
/** A pane body: counts its mounts; a tile draws the real `TileFrame` (its header is the drag handle). */
function Probe({ id }: { id: string }) {
  useEffect(() => {
    mounts[id] = (mounts[id] ?? 0) + 1
  }, [id])
  const body = h('div', { 'data-body': id }, id, h('button', { type: 'button' }, 'Show'))
  return id.startsWith('tile:') ? h(TileFrame, { title: id, onClose: () => undefined, children: body }) : body
}

beforeEach(() => {
  Object.assign(globalThis, { AbortController: window.AbortController, AbortSignal: window.AbortSignal })
  localStorage.removeItem(STORAGE_KEY)
  for (const k of Object.keys(mounts)) delete mounts[k]
  navigate.mockClear()
})
afterEach(() => {
  untile()
  document.body.innerHTML = ''
})

const mount = async (session: string) => {
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  const tree = (s: string) =>
    h(QueryClientProvider, { client: new QueryClient() }, h(SidebarProvider, null, h(Panes, { session: s, kinds: ['jobs', 'prompt'], render: (id: string) => h(Probe, { id }) })))
  await act(async () => root.render(tree(session)))
  return { host, root, pane: (leaf: string) => host.querySelector<HTMLElement>(`[data-pane="${leaf}"]`)! }
}
const press = (el: Element) => el.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true, button: 0 }))

test('opening, moving, docking and closing panes never remounts another; every leaf kind is a Trellis view', async () => {
  const { root } = await mount('a')
  expect(usePanes.getState().leaves).toEqual(['a'])
  const jobs = tileLeaf(tileId('jobs', 'a'))
  const prompt = tileLeaf(tileId('prompt', 'a'))
  await act(async () => void openTile({ kind: 'jobs', host: 'a', props: {} }))
  await act(async () => void openTile({ kind: 'prompt', host: 'a', props: {} }))
  expect(usePanes.getState().leaves).toEqual(['a', jobs, prompt])
  const ws = usePanes.getState().ws!
  for (const edge of ['left', 'top', 'right', 'bottom'] as const) await act(async () => ws.dock(prompt, { beside: ws.view('a')!.panelId, edge }))
  await act(async () => ws.dock(jobs, { into: ws.view(prompt)!.panelId }))
  await act(async () => usePanes.getState().place('a', 'b', 'right'))
  await act(async () => closeTile(tileId('jobs', 'a')))
  expect(usePanes.getState().leaves).toEqual(expect.arrayContaining(['a', 'b', prompt]))
  expect(mounts).toEqual({ a: 1, b: 1, [jobs]: 1, [prompt]: 1 })
  await act(async () => root.unmount())
})

test('a press selects a tile without touching focus or the URL; a control pressed in a chat keeps the tile selected', async () => {
  const { root, pane } = await mount('a')
  const jobs = tileLeaf(tileId('jobs', 'a'))
  await act(async () => void openTile({ kind: 'jobs', host: 'a', props: {} }))
  await act(async () => {
    usePanes.getState().place('a', 'b', 'bottom')
    usePanes.getState().focus('a')
  })
  navigate.mockClear()
  await act(async () => press(pane(jobs).querySelector('[data-body]')!))
  expect(usePanes.getState()).toMatchObject({ focused: 'a', selected: jobs })
  expect(pane(jobs).className).not.toContain('opacity-80')
  expect(pane('a').className).toContain('opacity-80')
  // "Show" in another chat: the tile stays selected, so what it opens takes the tile's pane.
  await act(async () => press(pane('b').querySelector('button')!))
  expect(usePanes.getState()).toMatchObject({ focused: 'a', selected: jobs })
  await act(async () => void openTile({ kind: 'prompt', host: 'b', props: {} }))
  // The tile's pane (right of the a/b column) now shows the new surface: a, b, then it.
  expect(usePanes.getState().leaves).toEqual(['a', 'b', tileLeaf(tileId('prompt', 'b'))])
  expect(usePanes.getState().leaves).not.toContain(jobs)
  expect(navigate).not.toHaveBeenCalled()
  // A press on a chat's body selects and focuses it, and moves the URL there.
  await act(async () => press(pane('b').querySelector('[data-body]')!))
  expect(usePanes.getState()).toMatchObject({ focused: 'b', selected: 'b' })
  expect(navigate).toHaveBeenCalledWith({ to: '/c/$session', params: { session: 'b' } })
  await act(async () => root.unmount())
})

test("beside other panes a tile header is no window drag region and hands its press to the view's Trellis tab; alone it moves the window", async () => {
  const { root, pane } = await mount('a')
  const jobs = tileLeaf(tileId('jobs', 'a'))
  await act(async () => void openTile({ kind: 'jobs', host: 'a', props: {} }))
  const header = pane(jobs).querySelector<HTMLElement>('header')!
  expect(header.hasAttribute('data-tauri-drag-region')).toBe(false)
  const tab = [...document.querySelectorAll<HTMLElement>('[data-trellis-part="tab"]')].find((t) => t.dataset.view === jobs)!
  const pressed = vi.fn()
  tab.addEventListener('pointerdown', pressed)
  await act(async () => press(header.querySelector('span')!))
  expect(pressed).toHaveBeenCalledOnce()
  // A press on a header control stays the control's.
  await act(async () => press(header.querySelector('button')!))
  expect(pressed).toHaveBeenCalledOnce()
  await act(async () => usePanes.getState().close('a'))
  expect(usePanes.getState().leaves).toEqual([jobs])
  expect(header.hasAttribute('data-tauri-drag-region')).toBe(true)
  await act(async () => root.unmount())
})

test('the layout survives the panes unmounting (another page) and a sidebar drop on a tile’s centre joins it as a tab', async () => {
  const first = await mount('a')
  await act(async () => void openTile({ kind: 'jobs', host: 'a', props: {} }))
  await act(async () => first.root.unmount())
  const { root, pane } = await mount('a')
  const jobs = tileLeaf(tileId('jobs', 'a'))
  expect(usePanes.getState().leaves).toEqual(['a', jobs])
  pane(jobs).getBoundingClientRect = () => ({ left: 0, top: 0, width: 400, height: 400, right: 400, bottom: 400, x: 0, y: 0, toJSON() {} })
  document.elementFromPoint = () => pane(jobs)
  await act(async () => {
    usePanes.getState().startDrag('b', 'B', 0, 0)
    usePanes.getState().moveDrag(200, 200)
  })
  expect(usePanes.getState().drag?.target?.edge).toBe('tab')
  await act(async () => void usePanes.getState().endDrag(true))
  expect(usePanes.getState().leaves).toEqual(['a', jobs, 'b'])
  expect(usePanes.getState().ws!.view('b')!.panelId).toBe(usePanes.getState().ws!.view(jobs)!.panelId)
  // Once per page mount; the drop mounts only `b`.
  expect(mounts).toEqual({ a: 2, [jobs]: 2, b: 1 })
  await act(async () => root.unmount())
})
