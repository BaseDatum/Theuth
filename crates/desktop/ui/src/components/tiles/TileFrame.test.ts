import '../settings/test-dom'
import type * as Popout from '@/lib/popout'
import { act, createElement as h } from 'react'
import { createRoot } from 'react-dom/client'
import { toast } from 'sonner'
import { afterEach, expect, test, vi } from 'vitest'
import { SidebarProvider } from '@/components/ui/sidebar'
import { routed } from '@/lib/owner'
import { tileLeaf, usePanes } from '@/lib/panes'
import { closeTile } from '@/lib/tiles'
import { popOutAndClose, TileFrame } from './TileFrame'

vi.mock('@/lib/popout', async (original) => ({ ...(await original<typeof Popout>()), canPopout: () => true, isPopout: () => false }))
vi.mock('sonner', () => ({ toast: { error: vi.fn() } }))
// Every session action (archive, delete, stop, move…) goes through `routed`: a pop-out must never reach it.
vi.mock('@/lib/owner', async (original) => ({ ...(await original<object>()), routed: vi.fn() }))

let unmount = async () => {}
afterEach(async () => {
  await unmount()
  document.body.innerHTML = ''
  usePanes.setState({ leaves: [], views: {}, focused: null })
  vi.clearAllMocks()
})

async function frame(popOut: () => Promise<unknown>, onClose: () => void) {
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  await act(async () => root.render(h(SidebarProvider, null, h(TileFrame, { title: 'Diff', popOut, onClose, children: 'body' }))))
  unmount = async () => act(async () => root.unmount())
  return host.querySelector<HTMLButtonElement>('button[aria-label="Open in new window"]')!
}

test('Open in new window closes the tile once its window opened, by its own Close', async () => {
  let opened!: () => void
  const onClose = vi.fn()
  const button = await frame(() => new Promise<void>((r) => (opened = r)), onClose)
  await act(async () => button.click())
  // Not before the window is there: a failure must find the tile still open.
  expect(onClose).not.toHaveBeenCalled()
  await act(async () => opened())
  expect(onClose).toHaveBeenCalledTimes(1)
})

test('a window that fails to open leaves the tile open and says so', async () => {
  const onClose = vi.fn()
  const button = await frame(() => Promise.reject(new Error('no window')), onClose)
  await act(async () => button.click())
  expect(onClose).not.toHaveBeenCalled()
  expect(toast.error).toHaveBeenCalledWith('Could not open a window: no window')
})

test('popped out, the tile leaves the pane tree through the Close path; its chat stays and no session call is made', async () => {
  const tile = tileLeaf('diff:s1:')
  usePanes.setState({ leaves: ['s1', tile], views: {}, focused: 's1' })
  expect(await popOutAndClose(async () => {}, () => closeTile('diff:s1:'))).toBe(true)
  expect(usePanes.getState().leaves).toEqual(['s1'])
  expect(usePanes.getState().focused).toBe('s1')

  // A chat pane popped out beside another closes the same way (`usePanes.close`, its Close button's call): the pane
  // alone; the session it showed is left as it was.
  usePanes.setState({ leaves: ['s1', 's2'], focused: 's2' })
  expect(await popOutAndClose(async () => {}, () => usePanes.getState().close('s2'))).toBe(true)
  expect(usePanes.getState().leaves).toEqual(['s1'])
  expect(usePanes.getState().focused).toBe('s1')

  // A failed window leaves the tree as it was.
  usePanes.setState({ leaves: ['s1', tile] })
  expect(await popOutAndClose(() => Promise.reject(new Error('x')), () => closeTile('diff:s1:'))).toBe(false)
  expect(usePanes.getState().leaves).toEqual(['s1', tile])
  expect(routed).not.toHaveBeenCalled()
})
