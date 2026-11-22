import '../settings/test-dom'
import { act, createElement as h, useEffect } from 'react'
import { createRoot } from 'react-dom/client'
import { expect, test, vi } from 'vitest'
import { SidebarProvider } from '@/components/ui/sidebar'
import { leaves, tileLeaf, usePanes } from '@/lib/panes'
import { closeTile, openTile, tileId } from '@/lib/tiles'
import { Panes } from './Panes'

const navigate = vi.hoisted(() => vi.fn())
vi.mock('@tanstack/react-router', async (original) => ({ ...(await original<object>()), useNavigate: () => navigate }))

const mounts: Record<string, number> = {}
function Probe({ id }: { id: string }) {
  useEffect(() => {
    mounts[id] = (mounts[id] ?? 0) + 1
  }, [id])
  return h('div', { 'data-body': id }, id, h('button', { type: 'button' }, 'Show'))
}

test('opening, moving, swapping and closing tiles never remounts another pane; leaves sit where the tree puts them', async () => {
  usePanes.setState({ root: null, focused: 'a', selected: null, drag: null })
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  await act(async () => root.render(h(SidebarProvider, null, h(Panes, { session: 'a', render: (id: string) => h(Probe, { id }) }))))
  const jobs = tileLeaf(tileId('jobs', 'a'))
  const prompt = tileLeaf(tileId('prompt', 'a'))
  await act(async () => void openTile({ kind: 'jobs', host: 'a', props: {} }))
  await act(async () => void openTile({ kind: 'prompt', host: 'a', props: {} }))
  expect(leaves(usePanes.getState().root!)).toEqual(['a', jobs, prompt])
  const box = (leaf: string) => {
    const s = host.querySelector<HTMLElement>(`[data-pane="${leaf}"]`)!.style
    return [s.left, s.top, s.width, s.height]
  }
  expect(box('a')).toEqual(['0%', '0%', '50%', '100%'])
  expect(box(prompt)).toEqual(['50%', '50%', '50%', '50%'])
  for (const edge of ['left', 'top', 'right', 'bottom', 'center'] as const) await act(async () => usePanes.getState().move(prompt, 'a', edge))
  await act(async () => usePanes.getState().move(jobs, prompt, 'center'))
  await act(async () => closeTile(tileId('jobs', 'a')))
  expect(leaves(usePanes.getState().root!)).toEqual(['a', prompt])
  expect(mounts).toEqual({ a: 1, [jobs]: 1, [prompt]: 1 })
  await act(async () => root.unmount())
})

test('a press selects a tile without touching focus or the URL; a control pressed in a chat keeps the tile selected', async () => {
  usePanes.setState({ root: null, focused: 'a', selected: 'a', drag: null })
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  await act(async () => root.render(h(SidebarProvider, null, h(Panes, { session: 'a', render: (id: string) => h(Probe, { id }) }))))
  const jobs = tileLeaf(tileId('jobs', 'a'))
  await act(async () => void openTile({ kind: 'jobs', host: 'a', props: {} }))
  await act(async () => {
    usePanes.getState().place('a', 'b', 'bottom')
    usePanes.getState().focus('a')
  })
  navigate.mockClear()
  const press = (el: Element) => el.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true, button: 0 }))
  const pane = (leaf: string) => host.querySelector<HTMLElement>(`[data-pane="${leaf}"]`)!
  await act(async () => press(pane(jobs).querySelector('[data-body]')!))
  expect(usePanes.getState()).toMatchObject({ focused: 'a', selected: jobs })
  expect(pane(jobs).className).not.toContain('opacity-80')
  expect(pane('a').className).toContain('opacity-80')
  // "Show" in another chat: the tile stays selected, so what it opens takes the tile's pane.
  await act(async () => press(pane('b').querySelector('button')!))
  expect(usePanes.getState()).toMatchObject({ focused: 'a', selected: jobs })
  await act(async () => void openTile({ kind: 'prompt', host: 'b', props: {} }))
  expect(leaves(usePanes.getState().root!)).toEqual(['a', 'b', tileLeaf(tileId('prompt', 'b'))])
  expect(navigate).not.toHaveBeenCalled()
  // A press on a chat's body selects and focuses it, and moves the URL there.
  await act(async () => press(pane('b').querySelector('[data-body]')!))
  expect(usePanes.getState()).toMatchObject({ focused: 'b', selected: 'b' })
  expect(navigate).toHaveBeenCalledWith({ to: '/c/$session', params: { session: 'b' } })
  await act(async () => root.unmount())
})
