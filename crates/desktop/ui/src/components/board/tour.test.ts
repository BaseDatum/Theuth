import { expect, test, vi } from 'vitest'
import { activeStopForScroll, tourKeyIndex } from './Tour'

// `Tour.tsx` pulls the diagram views (theme, tooltips, connection); only the pure rules are under test here.
vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { message: vi.fn(), error: vi.fn() }) }))
vi.mock('@/lib/rpc', () => ({ useApi: () => ({}) }))
vi.mock('@/components/ui/tooltip', () => ({ Tooltip: ({ children }: { children: unknown }) => children, TooltipTrigger: ({ children }: { children: unknown }) => children, TooltipContent: () => null }))
vi.mock('@/lib/theme', () => ({ subscribeDark: () => () => {} }))
vi.mock('@/lib/diagram-theme', () => ({ diagramTokens: () => ({}) }))

// R §B.7 `activeTargetForScroll`: the first stop at or below the scroller top is active when it sits in the top half.
test('scroll-sync picks the first stop below the top when it is in the top half, else the previous one', () => {
  const tops = [0, 300, 600, 900]
  expect(activeStopForScroll(tops, 0, 400)).toBe(0)
  expect(activeStopForScroll(tops, 300, 400)).toBe(1) // exactly at the top
  expect(activeStopForScroll(tops, 450, 400)).toBe(2) // 600 ≤ 450 + 200
  expect(activeStopForScroll(tops, 310, 400)).toBe(1) // 600 > 310 + 200 → the one before
  expect(activeStopForScroll(tops, 1000, 400)).toBe(3) // scrolled past every stop → the last
  expect(activeStopForScroll(tops, 50, 400)).toBe(0) // 300 > 250, previous of 1 is 0
  expect(activeStopForScroll([], 0, 400)).toBe(0)
})

test('tour keys: ↓/j next, ↑/k previous, Home/End, clamped; anything else is not a tour key', () => {
  expect(tourKeyIndex('ArrowDown', 0, 6)).toBe(1)
  expect(tourKeyIndex('j', 5, 6)).toBe(5)
  expect(tourKeyIndex('ArrowUp', 0, 6)).toBe(0)
  expect(tourKeyIndex('k', 3, 6)).toBe(2)
  expect(tourKeyIndex('Home', 4, 6)).toBe(0)
  expect(tourKeyIndex('End', 0, 6)).toBe(5)
  expect(tourKeyIndex('Enter', 0, 6)).toBeNull()
  expect(tourKeyIndex('Escape', 0, 6)).toBeNull()
})
