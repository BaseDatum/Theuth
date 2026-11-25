import '../components/settings/test-dom'
import { afterEach, expect, test } from 'vitest'
import { initAnimations, motionAllowed, useAnimations } from './animations'

afterEach(() => {
  localStorage.clear()
  useAnimations.getState().setEnabled(true)
})

test('Animations defaults to enabled, persists to localStorage, and mirrors data-animations="off" on <html>', () => {
  initAnimations()
  expect(useAnimations.getState().enabled).toBe(true)
  expect(motionAllowed()).toBe(true)
  expect(document.documentElement.getAttribute('data-animations')).toBeNull()

  useAnimations.getState().setEnabled(false)
  expect(localStorage.getItem('ui.animations')).toBe('false')
  expect(document.documentElement.getAttribute('data-animations')).toBe('off')
  expect(motionAllowed()).toBe(false)

  useAnimations.getState().setEnabled(true)
  expect(localStorage.getItem('ui.animations')).toBe('true')
  expect(document.documentElement.getAttribute('data-animations')).toBeNull()
  expect(motionAllowed()).toBe(true)
})
