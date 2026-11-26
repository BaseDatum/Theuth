import '../settings/test-dom'
import { act, createElement as h } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import { CostsResident } from './Resident'

const env = vi.hoisted(() => ({ pathname: '/c/chat' }))
vi.mock('@tanstack/react-router', () => ({
  useRouterState: ({ select }: { select(s: { location: { pathname: string; search: object } }): unknown }) => select({ location: { pathname: env.pathname, search: {} } }),
}))
vi.mock('@/lib/costs/queries', () => ({ useCostCube: () => ({ cells: { hour: [{}] } }) }))
vi.mock('./CostsPage', () => ({ CostsPage: () => null }))

// Ours is an own property of `HTMLElement.prototype` over jsdom's `Element.prototype.clientWidth`; deleting it restores that.
const proto = HTMLElement.prototype as { clientWidth?: number }
afterEach(() => {
  delete proto.clientWidth
  vi.useRealTimers()
  document.body.innerHTML = ''
})

test('hidden, the resident page keeps the width it had, so the column resizing under it lays nothing out; on /costs it takes the column again', async () => {
  vi.useFakeTimers()
  let column = 900
  Object.defineProperty(proto, 'clientWidth', { configurable: true, get: () => column })
  const root = createRoot(document.body.appendChild(document.createElement('div')))
  const go = (pathname: string) => act(async () => ((env.pathname = pathname), root.render(h(CostsResident))))
  await go('/c/chat')
  // Pre-rendered hidden once the cube is held.
  await act(async () => void vi.advanceTimersByTime(2_000))
  const box = document.querySelector<HTMLElement>('.absolute')!
  expect(box.hasAttribute('inert')).toBe(true)
  expect([box.style.width, box.style.right]).toEqual(['900px', 'auto'])
  // The sidebar toggles while away: the pinned width stays.
  column = 1100
  await go('/c/other')
  expect(box.style.width).toBe('900px')
  await go('/costs')
  expect(box.hasAttribute('inert')).toBe(false)
  expect([box.style.width, box.style.right]).toEqual(['', ''])
  // Leaving again pins the column's width as it is now.
  await go('/c/chat')
  expect([box.style.width, box.style.right]).toEqual(['1100px', 'auto'])
  await act(async () => root.unmount())
})
