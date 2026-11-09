import '../settings/test-dom'
import { act, createElement as h, type ReactElement, useEffect } from 'react'
import { createRoot } from 'react-dom/client'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, expect, test } from 'vitest'
import { readSidebarWidth, Sidebar, SidebarMenuAction, SidebarProvider, useSidebar } from './sidebar'

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

// jsdom's about:blank drops cookies: a minimal jar, one `name=value` per write like the browser's.
const jar = new Map<string, string>()
Object.defineProperty(document, 'cookie', {
  configurable: true,
  get: () => [...jar].map(([k, v]) => `${k}=${v}`).join('; '),
  set: (raw: string) => {
    const [k, v] = raw.split(';')[0]!.split('=')
    jar.set(k!.trim(), v!)
  },
})
const desktop = window.matchMedia
afterEach(() => {
  jar.clear()
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
