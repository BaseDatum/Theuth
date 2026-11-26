import '../settings/test-dom'
import { act, createElement as h, type ReactElement, useEffect } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, describe, expect, test } from 'vitest'
import {
  readSidebarOpen,
  readSidebarWidth,
  Sidebar,
  SidebarGroupLabel,
  SidebarInset,
  SidebarMenuAction,
  SidebarMenuButton,
  SidebarProvider,
  useSidebar,
  useSidebarActions,
  useSidebarIsMobile,
  useSidebarOpen,
  writeSidebarOpen,
} from './sidebar'
import { cornerPad, headerToggle } from '@/lib/sidebar-corner'

const classes = (showOnHover: boolean) => /class="([^"]*)"/.exec(renderToStaticMarkup(h(SidebarMenuAction, { showOnHover })))![1]!.split(' ')

test('the ⋯ hides until hover only for a fine pointer at ≥ md; the finger hit area is dropped only there too', () => {
  const on = classes(true)
  expect(on).toContain('md:pointer-fine:opacity-0')
  expect(on).not.toContain('md:opacity-0')
  expect(on).toContain('group-hover/menu-item:opacity-100')
  expect(on).toContain('after:-inset-2')
  expect(on).toContain('md:pointer-fine:after:hidden')
  expect(on).not.toContain('md:after:hidden')
  expect(classes(false)).not.toContain('md:pointer-fine:opacity-0')
})

// jsdom's about:blank drops cookies: a minimal jar, one `name=value` per write like the browser's; `writes` logs each name.
const jar = new Map<string, string>()
const writes: string[] = []
Object.defineProperty(document, 'cookie', {
  configurable: true,
  get: () => [...jar].map(([k, v]) => `${k}=${v}`).join('; '),
  set: (raw: string) => {
    const [k, v] = raw.split(';')[0]!.split('=')
    jar.set(k!.trim(), v!)
    writes.push(k!.trim())
  },
})
const desktop = window.matchMedia
afterEach(() => {
  jar.clear()
  writes.length = 0
  window.matchMedia = desktop
  document.body.innerHTML = ''
})

const mount = async (el: ReactElement) => {
  const host = document.createElement('div')
  document.body.append(host)
  const root = createRoot(host)
  await act(async () => root.render(el))
  const handle = () => document.querySelector<HTMLElement>('[role="separator"]')
  const width = () => host.querySelector<HTMLElement>('[data-slot="sidebar-wrapper"]')!.style.getPropertyValue('--sidebar-width')
  const fire = (target: EventTarget, e: Event) => act(async () => void target.dispatchEvent(e))
  const key = (k: string, init: KeyboardEventInit = {}) => new KeyboardEvent('keydown', { key: k, bubbles: true, ...init })
  return { root, handle, width, fire, key }
}
const shell = (props: { open?: boolean } = {}, ...extra: ReactElement[]) => h(SidebarProvider, props, h(Sidebar, null, 'nav'), ...extra)

test('the width cookie restores the width, clamped to 12rem–28rem; none or garbage is 16rem', () => {
  expect(readSidebarWidth('')).toBe(256)
  expect(readSidebarWidth('sidebar_state=true; sidebar_width=300')).toBe(300)
  expect(readSidebarWidth('sidebar_width=50')).toBe(192)
  expect(readSidebarWidth('sidebar_width=9999')).toBe(448)
  expect(readSidebarWidth('xsidebar_width=300; sidebar_width=abc')).toBe(256)
})

test('the handle: a labelled vertical separator; arrows nudge 16 px and clamp, double-click resets, every change is saved', async () => {
  jar.set('sidebar_width', '300')
  const { root, handle, width, fire, key } = await mount(shell())
  expect(width()).toBe('300px')
  const sep = handle()!
  expect(sep.getAttribute('aria-label')).toBe('Resize sidebar')
  expect(sep.getAttribute('aria-orientation')).toBe('vertical')
  expect([sep.getAttribute('aria-valuenow'), sep.getAttribute('aria-valuemin'), sep.getAttribute('aria-valuemax')]).toEqual(['300', '192', '448'])
  await fire(sep, key('ArrowRight'))
  expect(width()).toBe('316px')
  expect(jar.get('sidebar_width')).toBe('316')
  for (let i = 0; i < 20; i++) await fire(sep, key('ArrowLeft'))
  expect(width()).toBe('192px')
  expect(handle()!.getAttribute('aria-valuenow')).toBe('192')
  await fire(sep, new MouseEvent('dblclick', { bubbles: true }))
  expect(width()).toBe('256px')
  expect(jar.get('sidebar_width')).toBe('256')
  await act(async () => root.unmount())
})

test('collapsing hides the handle and keeps the width; ⌘B still toggles with the handle focused', async () => {
  const { root, handle, width, fire, key } = await mount(shell())
  await fire(handle()!, key('ArrowRight'))
  handle()!.focus()
  await fire(handle()!, key('b', { metaKey: true }))
  expect(handle()).toBeNull()
  expect(width()).toBe('272px')
  await fire(window, key('b', { metaKey: true }))
  expect(handle()!.getAttribute('aria-valuenow')).toBe('272')
  await act(async () => root.unmount())
  // Mounted collapsed: no handle either.
  const closed = await mount(shell({ open: false }))
  expect(closed.handle()).toBeNull()
  await act(async () => closed.root.unmount())
})

test('phones: the open off-canvas Sheet has no handle', async () => {
  window.matchMedia = ((q: string) => ({ ...desktop(q), matches: true })) as typeof window.matchMedia
  const OpenSheet = () => {
    const { setOpenMobile } = useSidebar()
    useEffect(() => setOpenMobile(true), [setOpenMobile])
    return null
  }
  const { root, handle } = await mount(shell({}, h(OpenSheet)))
  expect(document.querySelector('[data-mobile="true"]')).not.toBeNull()
  expect(handle()).toBeNull()
  await act(async () => root.unmount())
})

test('expanded or rail is kept in the sidebar_state cookie; none (or garbage) is expanded', () => {
  expect(readSidebarOpen('')).toBe(true)
  expect(readSidebarOpen('sidebar_state=false; sidebar_width=300')).toBe(false)
  expect(readSidebarOpen('sidebar_width=300; sidebar_state=true')).toBe(true)
  writeSidebarOpen(false)
  expect(readSidebarOpen(document.cookie)).toBe(false)
  writeSidebarOpen(true)
  expect(readSidebarOpen(document.cookie)).toBe(true)
})

test('a shell with a rail: collapsing shows the rail (no header toggle, only the lights\' overhang); without one it hides', async () => {
  let seen: ReturnType<typeof useSidebar> | undefined
  const Probe = () => ((seen = useSidebar()), null)
  const { root } = await mount(h(SidebarProvider, { rail: true, defaultOpen: false }, h(Sidebar, { collapsible: 'icon' }, 'nav'), h(Probe)))
  expect(seen!.rail).toBe(true)
  expect(headerToggle(seen!)).toBe(false)
  expect(cornerPad(seen!)).toBe('pl-6')
  expect(document.querySelector('[data-slot="sidebar"]')!.getAttribute('data-collapsible')).toBe('icon')
  await act(async () => seen!.toggleSidebar())
  expect(seen!.rail).toBe(false)
  expect(cornerPad(seen!)).toBeUndefined()
  await act(async () => root.render(h(SidebarProvider, { key: 'plain', defaultOpen: false }, h(Sidebar, null, 'nav'), h(Probe))))
  expect(seen!.rail).toBe(false)
  expect(headerToggle(seen!)).toBe(true)
  expect(cornerPad(seen!)).toBe('pl-20')
  await act(async () => root.unmount())
})

// A mesh switch remounts the shell (App's `EngineProvider key={engine}`); `Mesh` stands in for it, `useSidebarOpen` for Root.
// The jar is emptied before each switch: the iOS/iPadOS webview keeps no cookie on the app's custom scheme.
describe('a mesh switch keeps the sidebar as it was', () => {
  let seen: ReturnType<typeof useSidebar> | undefined
  const Probe = () => ((seen = useSidebar()), null)
  const Shell = () => {
    const [open, setOpen] = useSidebarOpen()
    return h(SidebarProvider, { open, onOpenChange: setOpen, rail: true }, h(Sidebar, null, 'nav'), h(Probe))
  }
  const Mesh = ({ engine }: { engine: string }) => h(Shell, { key: engine })
  const switchTo = async (root: Root, engine: string) => {
    jar.clear()
    await act(async () => root.render(h(Mesh, { engine })))
  }

  test('collapsed (the rail) stays collapsed, expanded stays expanded', async () => {
    writeSidebarOpen(true)
    const { root } = await mount(h(Mesh, { engine: 'a' }))
    expect(seen!.state).toBe('expanded')
    await act(async () => seen!.toggleSidebar())
    expect(seen!.state).toBe('collapsed')
    await switchTo(root, 'b')
    expect(seen!.state).toBe('collapsed')
    expect(seen!.rail).toBe(true)
    await switchTo(root, 'c')
    expect(seen!.state).toBe('collapsed')
    await act(async () => seen!.toggleSidebar())
    // A stale cookie from an older page does not win over this page's choice either.
    jar.set('sidebar_state', 'false')
    await act(async () => root.render(h(Mesh, { engine: 'a' })))
    expect(seen!.state).toBe('expanded')
    await switchTo(root, 'b')
    expect(seen!.state).toBe('expanded')
    await act(async () => root.unmount())
  })

  test('phones: a closed Sheet stays closed and the desktop preference is untouched', async () => {
    window.matchMedia = ((q: string) => ({ ...desktop(q), matches: true })) as typeof window.matchMedia
    writeSidebarOpen(false)
    const { root } = await mount(h(Mesh, { engine: 'a' }))
    expect([seen!.isMobile, seen!.openMobile, seen!.open]).toEqual([true, false, false])
    await switchTo(root, 'b')
    expect([seen!.openMobile, seen!.open]).toEqual([false, false])
    expect(document.querySelector('[data-mobile="true"]')).toBeNull()
    writeSidebarOpen(true)
    await switchTo(root, 'c')
    expect([seen!.openMobile, seen!.open]).toEqual([false, true])
    await act(async () => root.unmount())
  })
})

// iPad (touch, not a phone): a leftward swipe collapses only from inside the sidebar itself, whatever its width — it
// used to be any start left of 300 px, so an Inbox card's swipe-left (snooze) beside a 256 px sidebar collapsed it
// too — and never from a row with its own swipe (a session row's swipe-left archives).
describe('the touch edge swipe', () => {
  let seen: ReturnType<typeof useSidebar> | undefined
  const Probe = () => ((seen = useSidebar()), null)
  const swipe = async (target: Element, from: number, to: number) => {
    const ev = (type: string, x: number) => {
      const e = new Event(type, { bubbles: true })
      Object.defineProperties(e, { pointerType: { value: 'touch' }, isPrimary: { value: true }, clientX: { value: x }, clientY: { value: 100 } })
      return e
    }
    await act(async () => {
      target.dispatchEvent(ev('pointerdown', from))
      target.dispatchEvent(ev('pointermove', (from + to) / 2))
      target.dispatchEvent(ev('pointermove', to))
      target.dispatchEvent(ev('pointerup', to))
    })
  }
  const ipad = async () => {
    window.matchMedia = ((q: string) => ({ ...desktop(q), matches: q.includes('coarse') })) as typeof window.matchMedia
    return mount(
      h(
        SidebarProvider,
        { rail: true, defaultOpen: true },
        h(Sidebar, { collapsible: 'icon' }, h('div', { 'data-row': '' }, 'row'), h('div', { 'data-swipe': '', 'data-swiped-row': '' }, 'swipeable')),
        h('main', { 'data-card': '' }, 'card'),
        h(Probe),
      ),
    )
  }

  test('a swipe-left that starts outside the sidebar (an Inbox card beside it) leaves it open, even within 300 px', async () => {
    const { root } = await ipad()
    await swipe(document.querySelector('[data-card]')!, 280, 100)
    expect(seen!.state).toBe('expanded')
    await act(async () => root.unmount())
  })

  test('a swipe-left that starts on a row with its own swipe leaves it to the row', async () => {
    const { root } = await ipad()
    await swipe(document.querySelector('[data-swiped-row]')!, 200, 40)
    expect(seen!.state).toBe('expanded')
    await act(async () => root.unmount())
  })

  test('a swipe-left from inside the sidebar collapses it to the rail; one from the left edge expands it again', async () => {
    const { root } = await ipad()
    await swipe(document.querySelector('[data-row]')!, 200, 40)
    expect(seen!.state).toBe('collapsed')
    await swipe(document.querySelector('[data-card]')!, 10, 200)
    expect(seen!.state).toBe('expanded')
    await act(async () => root.unmount())
  })
})

test('the desktop sidebar animates no layout property: no width, left/right, margin or padding transitions', async () => {
  const { root } = await mount(h(SidebarProvider, { rail: true }, h(Sidebar, { collapsible: 'icon' }, 'nav'), h(SidebarInset)))
  const cls = (slot: string) => document.querySelector<HTMLElement>(`[data-slot="${slot}"]`)!.className
  for (const slot of ['sidebar-gap', 'sidebar-container']) expect(cls(slot)).not.toMatch(/transition-\[[^\]]*(width|left|right)/)
  expect(cls('sidebar-wrapper').split(' ')).toContain('overflow-x-clip')
  const markup = renderToStaticMarkup(h(SidebarProvider, null, h(SidebarMenuButton, null, 'Chat'), h(SidebarGroupLabel, null, 'Desks')))
  expect(markup).not.toMatch(/transition-\[[^\]]*(width|height|padding|margin)/)
  await act(async () => root.unmount())
})

test('actions and isMobile readers do not re-render on a toggle or a width change; the actions stay the same object', async () => {
  const renders = { actions: 0, mobile: 0, all: 0 }
  const seen: ReturnType<typeof useSidebarActions>[] = []
  const Actions = () => (renders.actions++, seen.push(useSidebarActions()), null)
  const Mobile = () => (renders.mobile++, useSidebarIsMobile(), null)
  let all: ReturnType<typeof useSidebar> | undefined
  const All = () => (renders.all++, (all = useSidebar()), null)
  const { root } = await mount(h(SidebarProvider, { rail: true, defaultOpen: true }, h(Sidebar, { collapsible: 'icon' }, 'nav'), h(Actions), h(Mobile), h(All)))
  const before = { ...renders }
  await act(async () => seen[0]!.toggleSidebar())
  await act(async () => seen[0]!.toggleSidebar())
  await act(async () => seen[0]!.setWidth(300))
  expect([renders.actions, renders.mobile]).toEqual([before.actions, before.mobile])
  expect(renders.all).toBe(before.all + 3)
  expect(all!.width).toBe(300)
  expect(new Set(seen).size).toBe(1)
  await act(async () => root.unmount())
})

test('a toggle saves sidebar_state exactly once, uncontrolled or controlled by useSidebarOpen', async () => {
  let seen: ReturnType<typeof useSidebar> | undefined
  const Probe = () => ((seen = useSidebar()), null)
  const free = await mount(h(SidebarProvider, { rail: true }, h(Sidebar, { collapsible: 'icon' }, 'nav'), h(Probe)))
  writes.length = 0
  await act(async () => seen!.toggleSidebar())
  expect(writes.filter((k) => k === 'sidebar_state')).toHaveLength(1)
  await act(async () => free.root.unmount())

  const Shell = () => {
    const [open, setOpen] = useSidebarOpen()
    return h(SidebarProvider, { open, onOpenChange: setOpen, rail: true }, h(Sidebar, { collapsible: 'icon' }, 'nav'), h(Probe))
  }
  const held = await mount(h(Shell))
  writes.length = 0
  await act(async () => seen!.toggleSidebar())
  expect(writes.filter((k) => k === 'sidebar_state')).toHaveLength(1)
  await act(async () => held.root.unmount())
})

test('⇧⌘S (`theuth:toggle-sidebar`) toggles the uncontrolled shell, which starts from the saved state; a controlled one ignores it', async () => {
  let seen: ReturnType<typeof useSidebar> | undefined
  const Probe = () => ((seen = useSidebar()), null)
  writeSidebarOpen(false)
  const free = await mount(h(SidebarProvider, { rail: true }, h(Sidebar, { collapsible: 'icon' }, 'nav'), h(Probe)))
  expect(seen!.state).toBe('collapsed')
  await act(async () => void window.dispatchEvent(new Event('theuth:toggle-sidebar')))
  expect(seen!.state).toBe('expanded')
  expect(readSidebarOpen(document.cookie)).toBe(true)
  await act(async () => free.root.unmount())
  const fixed = await mount(h(SidebarProvider, { open: true }, h(Probe)))
  await act(async () => void window.dispatchEvent(new Event('theuth:toggle-sidebar')))
  expect(seen!.open).toBe(true)
  await act(async () => fixed.root.unmount())
})

describe('the desktop slide', () => {
  type Fake = { el: Element; frames: Keyframe[]; cancelled: boolean; finish(): Promise<void> }
  let made: Fake[] = []
  const proto = HTMLElement.prototype as { animate?: unknown }
  afterEach(() => {
    delete proto.animate
    made = []
  })
  const fakeAnimate = () => {
    proto.animate = function (this: Element, frames: Keyframe[]) {
      const { promise: finished, resolve } = Promise.withResolvers<void>()
      const a: Fake = { el: this, frames, cancelled: false, finish: () => act(async () => resolve()) }
      made.push(a)
      return { finished, cancel: () => void (a.cancelled = true) }
    }
  }
  const shell = async () => {
    let seen: ReturnType<typeof useSidebar> | undefined
    const Probe = () => ((seen = useSidebar()), null)
    const m = await mount(h(SidebarProvider, { rail: true, defaultOpen: false }, h(Sidebar, { collapsible: 'icon' }, 'nav'), h(SidebarInset), h(Probe)))
    const gap = document.querySelector<HTMLElement>('[data-slot="sidebar-gap"]')!
    const box = document.querySelector<HTMLElement>('[data-slot="sidebar-container"]')!
    const inset = document.querySelector<HTMLElement>('[data-slot="sidebar-inset"]')!
    return { ...m, gap, box, inset, toggle: () => act(async () => seen!.toggleSidebar()) }
  }
  const xs = (a: Fake) => a.frames.map((f) => f.transform)

  test('expanding slides sidebar and column by transform alone, keeping the rail\'s gap until it ends; then the gap commits once', async () => {
    fakeAnimate()
    const { root, gap, box, inset, toggle } = await shell()
    expect(box.style.transform).toContain('var(--sidebar-width-icon) - var(--sidebar-width)')
    await toggle()
    // 256 px wide, the rail 56: both travel 200 px.
    expect(made.map((a) => [a.el, xs(a)])).toEqual([
      [box, ['translateX(-200px)', 'translateX(0px)']],
      [inset, ['translateX(0px)', 'translateX(200px)']],
    ])
    expect(gap.style.width).toBe('var(--sidebar-width-icon)')
    expect(box.style.transform).toBe('')
    await made[0]!.finish()
    expect(gap.style.width).toBe('')
    expect(made.every((a) => a.cancelled)).toBe(true)
    await act(async () => root.unmount())
  })

  test('collapsing commits the narrow gap at once and slides the wider column in; a toggle mid-slide drops the old slide for good', async () => {
    fakeAnimate()
    const { root, gap, box, inset, toggle } = await shell()
    await toggle()
    const opening = [...made]
    await toggle()
    expect(opening.every((a) => a.cancelled)).toBe(true)
    expect(gap.style.width).toBe('')
    const closing = made.slice(2)
    expect(closing.map((a) => a.el)).toEqual([box, inset])
    expect(xs(closing[1]!).at(-1)).toBe('translateX(0px)')
    expect(xs(closing[0]!).at(-1)).toBe('translateX(-200px)')
    // The stale slide finishing late leaves the collapsed gap alone.
    await opening[0]!.finish()
    expect(gap.style.width).toBe('')
    expect(closing.some((a) => a.cancelled)).toBe(false)
    await act(async () => root.unmount())
  })

  test('reduced motion (or no Web Animations, as here by default) jumps straight to the end', async () => {
    const { root, gap, toggle } = await shell()
    await toggle()
    expect(gap.style.width).toBe('')
    fakeAnimate()
    window.matchMedia = ((q: string) => ({ ...desktop(q), matches: q.includes('reduce') })) as typeof window.matchMedia
    await toggle()
    await toggle()
    expect(made).toEqual([])
    expect(gap.style.width).toBe('')
    await act(async () => root.unmount())
  })
})
