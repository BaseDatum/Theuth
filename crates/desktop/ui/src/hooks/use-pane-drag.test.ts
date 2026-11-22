import '../components/settings/test-dom'
import { act, createElement as h, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { TileHost } from '@/components/tiles/host'
import { TileFrame } from '@/components/tiles/TileFrame'
import { SidebarProvider } from '@/components/ui/sidebar'
import { leaves, usePanes } from '@/lib/panes'

vi.mock('@/lib/hotkeys', async (original) => ({ ...(await original<object>()), isMacDesktop: () => true }))

const T = 'tile:jobs:a:'
/** The pane a drop lands on: `a`, 400 px wide, so x 10 is its left band and x 200 its middle. */
const pane = document.createElement('div')
pane.setAttribute('data-pane', 'a')
pane.getBoundingClientRect = () => ({ left: 0, top: 0, width: 400, height: 400, right: 400, bottom: 400, x: 0, y: 0, toJSON() {} })

beforeEach(() => {
  usePanes.setState({ root: { dir: 'row', ratio: 0.5, a: { session: 'a' }, b: { session: T } }, focused: 'a', drag: null })
  document.elementFromPoint = () => pane
})
afterEach(() => {
  document.body.innerHTML = ''
})

const mount = async (leaf: string | null, controls: ReactNode = h('button', { 'aria-label': 'Tool' })) => {
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  const onClose = vi.fn()
  await act(async () =>
    root.render(h(SidebarProvider, null, h(TileHost.Provider, { value: leaf }, h(TileFrame, { title: 'Jobs', onClose, controls, children: 'body' })))),
  )
  const header = host.querySelector<HTMLElement>('header')!
  header.setPointerCapture = () => undefined
  header.hasPointerCapture = () => false
  return { host, root, header, onClose }
}

/** A mouse press on `el`, a move past the slop to (`x`, 200) (which starts the drag) and one more there (which
 * targets), the lift; returns the drag the store held mid-gesture. */
const drag = async (el: HTMLElement, x: number) => {
  const fire = (type: string, cx: number) =>
    el.dispatchEvent(Object.assign(new MouseEvent(type, { bubbles: true, button: 0, clientX: cx, clientY: 200 }), { pointerId: 1, pointerType: 'mouse' }))
  await act(async () => {
    fire('pointerdown', 300)
    fire('pointermove', x)
    fire('pointermove', x)
  })
  const mid = usePanes.getState().drag
  await act(async () => fire('pointerup', x))
  return mid
}

test('a split tree: the tile header is no window drag region and drags its own leaf; a drop on the left band moves it there', async () => {
  const { header, root } = await mount(T)
  expect(header.hasAttribute('data-tauri-drag-region')).toBe(false)
  expect(header.querySelector('[data-tauri-drag-region]')).toBeNull()
  const mid = await drag(header.querySelector('span')!, 10)
  expect(mid).toMatchObject({ session: T, title: 'Jobs', move: true, target: { session: 'a', edge: 'left' } })
  expect(usePanes.getState().drag).toBeNull()
  expect(leaves(usePanes.getState().root!)).toEqual([T, 'a'])
  expect(usePanes.getState().focused).toBe('a')
  await act(async () => root.unmount())
})

test('a centre drop swaps the tiles; the dragged tile never targets itself', async () => {
  const { header, root } = await mount(T)
  await drag(header, 200)
  expect(leaves(usePanes.getState().root!)).toEqual([T, 'a'])
  pane.setAttribute('data-pane', T)
  expect((await drag(header, 200))?.target).toBeNull()
  expect(leaves(usePanes.getState().root!)).toEqual([T, 'a'])
  pane.setAttribute('data-pane', 'a')
  await act(async () => root.unmount())
})

test('presses on header controls never start a drag; their clicks still land', async () => {
  const { host, header, root, onClose } = await mount(T)
  const close = host.querySelector<HTMLButtonElement>('button[aria-label="Close pane"]')!
  expect(close.closest('header')).toBe(header)
  expect(await drag(close, 10)).toBeNull()
  expect(await drag(host.querySelector<HTMLElement>('button[aria-label="Tool"]')!, 10)).toBeNull()
  expect(leaves(usePanes.getState().root!)).toEqual(['a', T])
  await act(async () => close.click())
  expect(onClose).toHaveBeenCalledOnce()
  await act(async () => root.unmount())
})

test('the single chat (no tree) and a header outside the panes keep moving the window, and never drag a tile', async () => {
  usePanes.setState({ root: null })
  const single = await mount('a')
  expect(single.header.getAttribute('data-tauri-drag-region')).toBe('true')
  expect(await drag(single.header, 10)).toBeNull()
  await act(async () => single.root.unmount())
  usePanes.setState({ root: { dir: 'row', ratio: 0.5, a: { session: 'a' }, b: { session: T } } })
  const outside = await mount(null)
  expect(outside.header.getAttribute('data-tauri-drag-region')).toBe('true')
  expect(await drag(outside.header, 10)).toBeNull()
  await act(async () => outside.root.unmount())
})

test('a lone frame or draft kept as a tree is one leaf: its header still moves the window', async () => {
  usePanes.setState({ root: { session: T } })
  const { header, root } = await mount(T)
  expect(header.getAttribute('data-tauri-drag-region')).toBe('true')
  expect(await drag(header, 10)).toBeNull()
  await act(async () => root.unmount())
})

test('presses inside the header\'s portaled popups (a Select list, a menu, a dialog) and on options or tabs never drag', async () => {
  const portaled = createPortal(
    h('div', { 'data-popup': '' }, h('span', { 'data-plain': '' }, 'v2'), h('button', { 'data-portal-button': '' }, 'Copy')),
    document.body,
  )
  const { host, header, root } = await mount(T, h('div', null, h('div', { role: 'option' }, 'v1'), h('div', { role: 'tab' }, 'Lens'), portaled))
  for (const el of [
    document.querySelector<HTMLElement>('[data-plain]')!,
    document.querySelector<HTMLElement>('[data-portal-button]')!,
    host.querySelector<HTMLElement>('[role=option]')!,
    host.querySelector<HTMLElement>('[role=tab]')!,
  ]) {
    el.setPointerCapture = () => undefined
    el.hasPointerCapture = () => false
    expect(await drag(el, 10)).toBeNull()
  }
  expect(leaves(usePanes.getState().root!)).toEqual(['a', T])
  expect(header.contains(document.querySelector('[data-popup]'))).toBe(false)
  await act(async () => root.unmount())
})

/** Press and move past the slop without lifting: the drag is live. */
const start = async (el: HTMLElement) => {
  const fire = (type: string, cx: number) =>
    el.dispatchEvent(Object.assign(new MouseEvent(type, { bubbles: true, button: 0, clientX: cx, clientY: 200 }), { pointerId: 1, pointerType: 'mouse' }))
  await act(async () => {
    fire('pointerdown', 300)
    fire('pointermove', 10)
    fire('pointermove', 10)
  })
}

test('Escape cancels a live tile drag: no ghost, no selection lock, nothing moved', async () => {
  const { header, root } = await mount(T)
  await start(header)
  expect(usePanes.getState().drag).toMatchObject({ session: T, move: true })
  expect(document.body.classList.contains('select-none')).toBe(true)
  await act(async () => window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' })))
  expect(usePanes.getState().drag).toBeNull()
  expect(document.body.classList.contains('select-none')).toBe(false)
  expect(leaves(usePanes.getState().root!)).toEqual(['a', T])
  await act(async () => root.unmount())
})

test('a header unmounted mid-drag, or a tree that stops being split, ends the drag', async () => {
  const first = await mount(T)
  await start(first.header)
  await act(async () => first.root.unmount())
  expect(usePanes.getState().drag).toBeNull()
  expect(document.body.classList.contains('select-none')).toBe(false)
  const second = await mount(T)
  await start(second.header)
  expect(usePanes.getState().drag).not.toBeNull()
  await act(async () => usePanes.setState({ root: { session: T } }))
  expect(usePanes.getState().drag).toBeNull()
  await act(async () => second.root.unmount())
})

test('a sidebar row dropped on the middle of a tile takes its pane over, as on a chat', async () => {
  pane.setAttribute('data-pane', T)
  usePanes.getState().startDrag('c', 'C', 0, 0)
  usePanes.getState().moveDrag(200, 200)
  expect(usePanes.getState().drag?.target).toMatchObject({ session: T, edge: 'center' })
  expect(usePanes.getState().endDrag(true)).toBe('c')
  expect(usePanes.getState().root).toMatchObject({ dir: 'row', ratio: 0.5, a: { session: 'a' }, b: { session: 'c' } })
  pane.setAttribute('data-pane', 'a')
})
