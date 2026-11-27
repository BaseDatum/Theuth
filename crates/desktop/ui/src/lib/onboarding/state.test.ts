import { beforeEach, expect, test, vi } from 'vitest'
import { ONBOARDING_DEFAULT, ONBOARDING_KEY, readOnboarding, useOnboarding } from './state'

const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => mem.get(k) ?? null,
  setItem: (k: string, v: string) => void mem.set(k, v),
})

beforeEach(() => {
  mem.clear()
  useOnboarding.getState().reset()
  useOnboarding.setState({ wizardOpen: false, completed: false })
})

test('garbage or absent storage reads as a fresh install', () => {
  expect(readOnboarding(null)).toEqual(ONBOARDING_DEFAULT)
  expect(readOnboarding('{not json')).toEqual(ONBOARDING_DEFAULT)
  expect(readOnboarding('42')).toEqual(ONBOARDING_DEFAULT)
})

test('stored values survive a round trip; bad parts fall back field by field; a stored step is ignored', () => {
  expect(readOnboarding(JSON.stringify({ step: 'first-chat', completed: true, checklistDismissed: true, visited: { costs: true }, runsOn: 'device' })))
    .toEqual({ completed: true, checklistDismissed: true, visited: { costs: true }, runsOn: 'device' })
  expect(readOnboarding(JSON.stringify({ completed: 'yes', visited: { inbox: true, settings: true, library: 1 }, runsOn: 'mars' })))
    .toEqual({ ...ONBOARDING_DEFAULT, visited: { inbox: true } })
})

test('actions persist the durable part and keep the session part out of storage', () => {
  const s = useOnboarding.getState()
  s.openWizard('model')
  s.markVisited('library')
  s.dismissChecklist()
  s.requestTour('main')
  const stored = JSON.parse(mem.get(ONBOARDING_KEY) ?? 'null')
  expect(stored).toEqual({ completed: false, checklistDismissed: true, visited: { library: true }, runsOn: null })
  expect(useOnboarding.getState()).toMatchObject({ step: 'model', wizardOpen: true, pendingTour: 'main' })
})

test('reset returns to Welcome, clears visits, dismissal and the pending tour; the flag mirror and the open wizard stay', () => {
  const s = useOnboarding.getState()
  s.openWizard('first-chat')
  s.markVisited('inbox')
  s.dismissChecklist()
  s.requestTour('costs')
  useOnboarding.setState({ completed: true })
  s.reset()
  expect(useOnboarding.getState()).toMatchObject({ step: 'welcome', checklistDismissed: false, visited: {}, completed: true, pendingTour: null, wizardOpen: true })
})

test('markVisited twice does not rewrite storage', () => {
  useOnboarding.getState().markVisited('costs')
  const spy = vi.spyOn(globalThis.localStorage, 'setItem')
  useOnboarding.getState().markVisited('costs')
  expect(spy).not.toHaveBeenCalled()
})
