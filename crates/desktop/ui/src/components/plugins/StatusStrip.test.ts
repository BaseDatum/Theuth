import '../settings/test-dom'
import { act, createElement as h } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { EMPTY_PLUGINS, pluginStore, seed } from '@/lib/plugins'
import { active } from '@/store/engine-scope'
import { STATUS_CAP, StatusStrip } from './StatusStrip'
import { placement, pluginInfo, snap } from './test-fixtures'

vi.mock('@/rpc/capabilities', () => ({ useQuerySupport: () => true }))
const layout = vi.hoisted(() => ({ mobile: false }))
vi.mock('@/hooks/use-mobile', () => ({ useIsMobile: () => layout.mobile }))

const store = pluginStore(active())
const status = (name: string, order: number) => pluginInfo(name, [placement(name, { kind: 'status', label: name, format: null })], { order })

beforeEach(() => {
  layout.mobile = false
  store.setState(seed(EMPTY_PLUGINS, 0, ['ping', 'turns', 'cpu', 'disk'].map(status), [
    snap('ping', 'ping', { label: '0.07 ms', tooltip: 'Round trip to the router' }),
    snap('disk', 'disk', { dot: 'error' }),
  ]))
})
afterEach(() => {
  document.body.innerHTML = ''
})

async function mount(variant?: 'bar' | 'sheet') {
  const host = document.body.appendChild(document.createElement('div'))
  const root = createRoot(host)
  await act(async () => root.render(h(StatusStrip, { variant })))
  return { host, unmount: () => act(async () => root.unmount()) }
}

test('the bar: three items with label and tooltip on hover, then an overflow lit by the error it holds', async () => {
  const { host, unmount } = await mount()
  const items = [...host.querySelectorAll<HTMLElement>('[data-plugin]')]
  expect(items).toHaveLength(STATUS_CAP)
  expect(items.map((i) => i.dataset.plugin)).toEqual(['ping', 'turns', 'cpu'])
  expect(items[0]?.title).toBe('ping · 0.07 ms · Round trip to the router')
  expect(items[1]?.querySelector('[data-dot]')?.getAttribute('data-dot')).toBe('off')
  // Icon and dot only: the label stays in the tooltip.
  expect(items[0]?.textContent).toBe('')
  const overflow = host.querySelector<HTMLButtonElement>('button[data-plugin-overflow]')!
  expect(overflow.getAttribute('aria-label')).toBe('1 more plugin')
  expect(overflow.querySelector('[data-dot]')?.getAttribute('data-dot')).toBe('error')
  await unmount()
})

test('on a phone the bar draws nothing; the sidebar sheet lists every item with its label', async () => {
  layout.mobile = true
  let m = await mount('bar')
  expect(m.host.innerHTML).toBe('')
  await m.unmount()
  m = await mount('sheet')
  expect([...m.host.querySelectorAll<HTMLElement>('[data-plugin]')].map((i) => i.textContent)).toEqual(['0.07 ms', 'turns', 'cpu', 'disk'])
  await m.unmount()
  layout.mobile = false
  m = await mount('sheet')
  expect(m.host.innerHTML).toBe('')
  await m.unmount()
})
