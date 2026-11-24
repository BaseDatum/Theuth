import { expect, test } from 'vitest'
import { presentSteps, settled, TOUR_GIVE_UP_MS, TOUR_WAIT_MS, TOURS, tourSelector } from './tours'

const anchors = (id: keyof typeof TOURS) => TOURS[id].steps.map((s) => s.anchor)

test('main tour: the contract order, ending on the checklist', () => {
  expect(anchors('main')).toEqual(['chat-composer', 'nav-inbox', 'nav-library', 'nav-costs', 'nav-agents', 'nav-settings', 'onboarding-checklist'])
  expect(TOURS.main.route).toBeNull()
})

test('focused tours open their page and highlight its anchors', () => {
  expect(TOURS.inbox).toMatchObject({ route: 'attention' })
  expect(anchors('inbox')).toEqual(['inbox-list'])
  expect(TOURS.library.route).toBe('library')
  expect(anchors('library')[0]).toBe('library-tabs')
  expect(TOURS.costs.route).toBe('costs')
  expect(anchors('costs')).toEqual(['costs-subscription-value', 'costs-limits'])
  expect(TOURS.devices.route).toBe('devices')
  expect(anchors('devices')).toEqual(['devices-pair'])
  expect(anchors('chat')).toEqual(['chat-composer', 'chat-model', 'chat-context'])
})

test('every step has short, concrete copy and a unique anchor within its tour', () => {
  for (const [id, tour] of Object.entries(TOURS)) {
    expect(new Set(tour.steps.map((s) => s.anchor)).size, id).toBe(tour.steps.length)
    for (const s of tour.steps) {
      expect(s.anchor, id).toMatch(/^[a-z]+(-[a-z]+)*$/)
      expect(s.title.length, s.anchor).toBeLessThanOrEqual(40)
      expect(s.body.length, s.anchor).toBeGreaterThan(40)
      expect(s.body.length, s.anchor).toBeLessThanOrEqual(260)
    }
  }
})

test('presentSteps keeps order, drops absent and skipped anchors', () => {
  const on = new Set(['chat-composer', 'nav-inbox', 'nav-costs', 'onboarding-checklist'])
  expect(presentSteps(TOURS.main.steps, (a) => on.has(a), ['onboarding-checklist']).map((s) => s.anchor)).toEqual(['chat-composer', 'nav-inbox', 'nav-costs'])
  expect(presentSteps(TOURS.main.steps, () => false)).toEqual([])
})

test('settled: all present at once; else the 4 s wait with something to show; else give up', () => {
  expect(settled(0, 7, 7)).toBe(true)
  expect(settled(TOUR_WAIT_MS - 1, 6, 7)).toBe(false)
  expect(settled(TOUR_WAIT_MS, 6, 7)).toBe(true)
  expect(settled(TOUR_WAIT_MS, 0, 7)).toBe(false)
  expect(settled(TOUR_GIVE_UP_MS, 0, 7)).toBe(true)
})

test('selector', () => {
  expect(tourSelector('nav-inbox')).toBe('[data-tour="nav-inbox"]')
})
