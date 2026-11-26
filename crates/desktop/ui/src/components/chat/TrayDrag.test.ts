import type * as Rpc from '@/lib/rpc'
import '../settings/test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement, type ComponentProps, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { TileHost } from '@/components/tiles/host'
import { openJobTerminalTile, showArtifact } from '@/components/tiles/open'
import { type DragPayload, makeTileId, payloadLeafAndSpec, tileLeaf, usePanes } from '@/lib/panes'
import { openPopout } from '@/lib/popout'
import { useWebViews } from '@/lib/web-views'
import { useSessions } from '@/store/sessions'
import { BackgroundArea } from './BackgroundArea'
import { artifactDrag, browserDrag, terminalDrag, whiteboardDrag } from './tray-drag'

// The iPad's tray: no windows (`canPopout` false, as `container() === 'ios'`), a tiling layout (a tile host), fingers.
const navigate = vi.fn()
vi.mock('@tanstack/react-router', () => ({
  useNavigate: () => navigate,
  Link: ({ to, params, ...props }: ComponentProps<'a'> & { to: string; params: Record<string, string> }) => createElement('a', { ...props, href: to.replace(/\$(\w+)/g, (_, key: string) => encodeURIComponent(params[key]!)) }),
}))
vi.mock('@/components/ui/sidebar', () => ({ useSidebar: () => ({ isMobile: false }) }))
vi.mock('@/rpc/capabilities', () => ({ useQuerySupport: () => true }))
const popout = vi.hoisted(() => ({ can: false }))
vi.mock('@/lib/popout', () => ({ POPOUT: null, canPopout: () => popout.can, openPopout: vi.fn(async () => {}), popoutOpen: vi.fn(async () => false) }))
vi.mock('@/rpc/discovery', async (original) => ({ ...(await original<object>()), supports: () => true }))
vi.mock('@/components/tiles/open', async (original) => ({
  ...(await original<object>()),
  showArtifact: vi.fn(),
  openJobTerminalTile: vi.fn(() => 'tile-id'),
}))
const api = { list_jobs: vi.fn(async () => []), list_terminals: vi.fn(async () => []), cell_state: vi.fn(async () => ({ cells: [], kernels: [] })) }
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => api }))

// jsdom has no pointer capture.
Object.assign(window.HTMLElement.prototype, { setPointerCapture() {}, releasePointerCapture() {}, hasPointerCapture: () => false })

const tick = (ms = 0) => new Promise<void>((resolve) => setTimeout(resolve, ms))
const run = (fn: () => unknown) => act(async () => { await fn(); await tick() })

function pointer(el: Element, type: string, x: number, y: number, pointerType = 'touch') {
  const e = new window.MouseEvent(type, { bubbles: true, cancelable: true, clientX: x, clientY: y, button: 0 })
  Object.defineProperties(e, { pointerType: { value: pointerType }, pointerId: { value: 1 } })
  el.dispatchEvent(e)
}
/** A finger: down, held past `HOLD_MS`, moved onto a pane's right edge, lifted. */
async function fingerDrag(el: Element) {
  await run(() => pointer(el, 'pointerdown', 10, 10))
  await act(async () => { await tick(450) })
  await run(() => pointer(el, 'pointermove', 400, 120))
  await run(() => pointer(el, 'pointerup', 400, 120))
}

const ended: (DragPayload | undefined)[] = []
const realPanes = usePanes.getState()
let unmount = async () => {}
let client: QueryClient
async function mount(element: ReactNode = createElement(TileHost.Provider, { value: 's1' }, createElement(BackgroundArea, { session: 's1' }))) {
  const host = document.createElement('div')
  document.body.append(host)
  const root = createRoot(host)
  client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
  await run(() => root.render(createElement(QueryClientProvider, { client, children: element })))
  unmount = () => run(() => root.unmount())
}
const art = (n: number) => ({ id: `a${n}`, session: 's1', call_id: `c${n}`, name: `a${n}.md`, title: `report ${n}`, kind: 'markdown' as const, mime: 'text/markdown', bytes: 10, version: 1, path: `/a${n}.md`, source: null, preview: null, pages: null, origin: null, created_at: n })

beforeEach(() => {
  vi.clearAllMocks()
  popout.can = false
  ended.length = 0
  useSessions.setState({ sessions: {} })
  // The panes store as the drag sees it: `moveDrag` snaps onto the right edge of pane `s1` (jsdom has no
  // `elementFromPoint`), `endDrag` records what the drop would place.
  usePanes.setState({
    ...realPanes,
    focused: 's1',
    drag: null,
    moveDrag: (x, y) => {
      const { drag } = usePanes.getState()
      if (drag) usePanes.setState({ drag: { ...drag, x, y, target: { session: 's1', edge: 'right', rect: new DOMRect(0, 0, 500, 500) } } })
    },
    endDrag: (drop) => {
      const { drag } = usePanes.getState()
      usePanes.setState({ drag: null })
      if (!drop || !drag?.target) return null
      ended.push(drag.payload)
      return drag.session
    },
  })
})
afterEach(async () => {
  await unmount()
  client?.clear()
  document.body.innerHTML = ''
  usePanes.setState(realPanes)
})

test('a finger held on an artifact dash, then dragged onto a pane, drops that artifact as a tile there; its click never fires', async () => {
  useSessions.getState().ensure('s1')
  useSessions.getState().setArtifacts('s1', [art(1)])
  await mount()
  const dash = document.querySelector('button[aria-label="Artifact report 1 · open"]')!
  expect(dash.getAttribute('data-drag-row')).toBe('true')
  await fingerDrag(dash)
  expect(ended).toEqual([{ kind: 'tile', tileKind: 'artifact', props: { callId: 'c1' }, title: 'report 1', session: 's1' }])
  // The click a finger's lift sends after a drag belongs to the drag.
  await run(() => (dash as HTMLButtonElement).click())
  expect(showArtifact).not.toHaveBeenCalled()
  // A tap (no hold) still opens it.
  await run(() => (dash as HTMLButtonElement).click())
  expect(showArtifact).toHaveBeenCalledWith('s1', 'c1')
})

test('a finger that lifts before the hold is a tap: nothing is dragged', async () => {
  useSessions.getState().ensure('s1')
  useSessions.getState().setArtifacts('s1', [art(1)])
  await mount()
  const dash = document.querySelector('button[aria-label="Artifact report 1 · open"]')!
  await run(() => pointer(dash, 'pointerdown', 10, 10))
  await run(() => pointer(dash, 'pointerup', 10, 10))
  await act(async () => { await tick(450) })
  expect(usePanes.getState().drag).toBeNull()
  expect(ended).toEqual([])
})

test('a mouse drag of an artifact dash drops it the same way (desktop)', async () => {
  useSessions.getState().ensure('s1')
  useSessions.getState().setArtifacts('s1', [art(1)])
  await mount()
  const dash = document.querySelector('button[aria-label="Artifact report 1 · open"]')!
  await run(() => pointer(dash, 'pointerdown', 10, 10, 'mouse'))
  await run(() => pointer(dash, 'pointermove', 40, 10, 'mouse'))
  await run(() => pointer(dash, 'pointermove', 400, 120, 'mouse'))
  await run(() => pointer(dash, 'pointerup', 400, 120, 'mouse'))
  expect(ended).toEqual([artifactDrag('s1', 'c1', 'report 1').payload])
})

test('iPad: tapping a terminal dash opens that terminal in a pane beside the chat (no window to pop out), never leaving it', async () => {
  useSessions.getState().ensure('s1')
  useSessions.setState((s) => ({ sessions: { ...s.sessions, s1: { ...s.sessions.s1!, ptys: new Set(['pty0']) } } }))
  await mount()
  const link = document.querySelector<HTMLAnchorElement>('a[aria-label^="Terminal pty0"]')!
  let prevented = false
  // After React's root listener: whether the link still navigates.
  document.addEventListener('click', (e) => { prevented = e.defaultPrevented }, { once: true })
  await run(() => link.click())
  expect(openJobTerminalTile).toHaveBeenCalledWith('s1', { session: 's1', job: 'pty0' }, 'Terminal pty0')
  expect(prevented).toBe(true)
  expect(openPopout).not.toHaveBeenCalled()
})

test('desktop: tapping a terminal dash still pops its window out', async () => {
  popout.can = true
  useSessions.getState().ensure('s1')
  useSessions.setState((s) => ({ sessions: { ...s.sessions, s1: { ...s.sessions.s1!, ptys: new Set(['pty0']) } } }))
  await mount()
  await run(() => document.querySelector<HTMLAnchorElement>('a[aria-label^="Terminal pty0"]')!.click())
  expect(openPopout).toHaveBeenCalledWith({ kind: 'pty', session: 's1', job: 'pty0' })
  expect(openJobTerminalTile).not.toHaveBeenCalled()
})

test('a finger drag of a terminal dash drops its terminal as a pane', async () => {
  useSessions.getState().ensure('s1')
  useSessions.setState((s) => ({ sessions: { ...s.sessions, s1: { ...s.sessions.s1!, ptys: new Set(['pty0']) } } }))
  await mount()
  await fingerDrag(document.querySelector('a[aria-label^="Terminal pty0"]')!)
  expect(ended).toEqual([terminalDrag('s1', { session: 's1', job: 'pty0' }, 'Terminal pty0').payload])
  expect(openJobTerminalTile).not.toHaveBeenCalled()
})

test('a finger drag of the opened tray\'s one-artifact tile drops that artifact as a pane', async () => {
  useSessions.getState().ensure('s1')
  useSessions.getState().setArtifacts('s1', [art(1)])
  await mount()
  await run(() => document.querySelector<HTMLButtonElement>('button[aria-label="Show background activity"]')!.click())
  const tile = document.querySelector('[data-tray-tiles] > button')!
  await fingerDrag(tile)
  expect(ended).toEqual([artifactDrag('s1', 'c1', 'report 1').payload])
})

test('a dragged item lands under the id its click opens it with, so dropping an open one moves it', () => {
  expect(payloadLeafAndSpec(artifactDrag('s1', 'c1', 'r').payload).leaf).toBe(tileLeaf(makeTileId('artifact', 's1')))
  expect(payloadLeafAndSpec(whiteboardDrag('s1', { doc: 'd1', title: 'W' }).payload).leaf).toBe(tileLeaf(makeTileId('whiteboard', 's1', 'd1')))
  const term = payloadLeafAndSpec(terminalDrag('s1', { session: 's2', job: 'j' }, 'T').payload)
  expect(term.leaf).toBe(tileLeaf(makeTileId('job-terminal', 's1', 's2/j')))
  expect(term.spec.params).toMatchObject({ session: 's1', props: { session: 's2', job: 'j', label: 'T' } })
})

test('a web page dragged from the tray is recorded as a web frame only when it drops', () => {
  const d = browserDrag('s1', { kind: 'web', id: 'w1', url: 'https://example.com', title: 'Example' })
  expect(d.payload).toEqual({ kind: 'chat', session: 'web:w1' })
  expect(useWebViews.getState().views.w1).toBeUndefined()
  d.prepare?.()
  expect(useWebViews.getState().views.w1).toMatchObject({ session: 's1', url: 'https://example.com' })
})
