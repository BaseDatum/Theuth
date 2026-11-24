import { expect, test } from 'vitest'
import { type BootInput, bootDecision, onboardingHere } from './detect'

const fresh: BootInput = { here: true, flagLoaded: true, completed: false, models: [], defaultModel: 'anthropic/claude-sonnet-4-5' }
const configured: BootInput = { ...fresh, models: [{ id: 'anthropic/claude-sonnet-4-5' }, { id: 'openai/gpt-5' }] }

test('a fresh install with no models opens the wizard', () => {
  expect(bootDecision(fresh)).toBe('wizard')
})

test('models present but the default model is not among them (its provider unconfigured) opens the wizard', () => {
  expect(bootDecision({ ...configured, defaultModel: 'google/gemini-2.5-pro' })).toBe('wizard')
  expect(bootDecision({ ...configured, defaultModel: '' })).toBe('wizard')
})

test('an upgrading user with a working default gets the tour toast, never the modal', () => {
  expect(bootDecision(configured)).toBe('toast')
})

test('a completed flag means nothing modal, whatever the models say', () => {
  expect(bootDecision({ ...fresh, completed: true })).toBe('none')
  expect(bootDecision({ ...configured, completed: true })).toBe('none')
})

test('nothing is decided before the flag and the model list answer', () => {
  expect(bootDecision({ ...fresh, flagLoaded: false })).toBe('wait')
  expect(bootDecision({ ...fresh, models: undefined })).toBe('wait')
  expect(bootDecision({ ...configured, defaultModel: undefined })).toBe('wait')
  // A completed flag needs no model list.
  expect(bootDecision({ ...fresh, completed: true, models: undefined })).toBe('none')
})

test('an unavailable model list degrades to nothing; elsewhere than the local shell nothing runs', () => {
  expect(bootDecision({ ...fresh, models: null })).toBe('none')
  expect(bootDecision({ ...fresh, here: false, flagLoaded: false })).toBe('none')
})

test('onboarding runs only in the desktop main window on its own engine', () => {
  const env = { hasLocal: true, active: 'local', local: 'local', popout: false, island: false, overlay: false }
  expect(onboardingHere(env)).toBe(true)
  expect(onboardingHere({ ...env, hasLocal: false })).toBe(false)
  expect(onboardingHere({ ...env, active: 'mesh-peer' })).toBe(false)
  expect(onboardingHere({ ...env, popout: true })).toBe(false)
  expect(onboardingHere({ ...env, island: true })).toBe(false)
  expect(onboardingHere({ ...env, overlay: true })).toBe(false)
})
