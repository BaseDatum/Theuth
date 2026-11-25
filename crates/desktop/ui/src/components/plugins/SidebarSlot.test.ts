import '../settings/test-dom'
import { act, createElement as h } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { EMPTY_PLUGINS, pluginStore, seed } from '@/lib/plugins'
import { active } from '@/store/engine-scope'
import { SIDEBAR_CAP, SidebarSlot } from './SidebarSlot'
import { placement, pluginInfo, snap } from './test-fixtures'

vi.mock('@/rpc/capabilities', () => ({ useQuerySupport: () => true }))
vi.mock('@/hooks/use-mobile', () => ({ useIsMobile: () => false }))

const store = pluginStore(active())
beforeEach(() => store.setState(EMPTY_PLUGINS))
afterEach(() => {
  document.body.innerHTML = ''
})

async function mount(variant: 'rail' | 'row') {
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  await act(async () => root.render(h(SidebarSlot, { variant })))
  return { host, unmount: () => act(async () => root.unmount()) }
}

test('six plugins: five icons in order, then an overflow button lit by the warning it holds; hidden placements are absent', async () => {
  const plugins = ['p0', 'p1', 'p2', 'p3', 'p4', 'p5'].map((name, order) =>
    pluginInfo(name, [placement(`${name}-icon`, { label: `${name} icon` }), placement(`${name}-hidden`, { hidden: true, label: `${name} hidden` })], { order }),
  )
  store.setState(seed(EMPTY_PLUGINS, 0, plugins, [snap('p5', 'p5-icon', { dot: 'warn' }), snap('p0', 'p0-icon', { badge: 4 })]))
  for (const variant of ['rail', 'row'] as const) {
    const { host, unmount } = await mount(variant)
    const icons = [...host.querySelectorAll<HTMLButtonElement>('button[data-plugin]')]
    expect(icons).toHaveLength(SIDEBAR_CAP)
    expect(icons.map((b) => b.getAttribute('aria-label'))).toEqual(['p0 icon', 'p1 icon', 'p2 icon', 'p3 icon', 'p4 icon'])
    expect(host.textContent).not.toContain('hidden')
    // The first icon's badge; dots off until a status arrives.
    expect(icons[0]?.textContent).toBe('4')
    expect(icons[1]?.querySelector('[data-dot]')?.getAttribute('data-dot')).toBe('off')
    const overflow = host.querySelector<HTMLButtonElement>('button[data-plugin-overflow]')!
    expect(overflow.getAttribute('aria-label')).toBe('1 more plugin')
    expect(overflow.querySelector('[data-dot]')?.getAttribute('data-dot')).toBe('warn')
    await unmount()
  }
})

test('five or fewer: no overflow button; an overflowed plugin that is fine leaves it unlit', async () => {
  const five = ['a', 'b', 'c', 'd', 'e'].map((name, order) => pluginInfo(name, [placement(name)], { order }))
  store.setState(seed(EMPTY_PLUGINS, 0, five, []))
  let m = await mount('row')
  expect(m.host.querySelectorAll('button[data-plugin]')).toHaveLength(5)
  expect(m.host.querySelector('button[data-plugin-overflow]')).toBeNull()
  await m.unmount()
  store.setState(seed(EMPTY_PLUGINS, 0, [...five, pluginInfo('f', [placement('f')], { order: 9 })], [snap('f', 'f', { dot: 'ok' })]))
  m = await mount('row')
  expect(m.host.querySelector('button[data-plugin-overflow] [data-dot]')).toBeNull()
  await m.unmount()
})

test('no sidebar placements: the slot draws nothing', async () => {
  store.setState(seed(EMPTY_PLUGINS, 0, [pluginInfo('s', [placement('s', { kind: 'status' })])], []))
  const { host, unmount } = await mount('rail')
  expect(host.innerHTML).toBe('')
  await unmount()
})
