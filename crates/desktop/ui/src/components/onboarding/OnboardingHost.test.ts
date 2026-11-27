// 224 on the iPhone/iPad: the first run asks "Where should theuth run?" once, before 215's setup wizard, where the
// engine can create cloud nodes; "Just this iPhone" continues to the setup wizard; desktop goes straight to it.
import '@/components/settings/test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement } from 'react'
import { type Root, createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import type * as Platform from '@/lib/platform'
import { ONBOARDING_KEY, useOnboarding } from '@/lib/onboarding/state'
import { OnboardingHost } from './OnboardingHost'

const env = vi.hoisted(() => ({ ios: true, cloud: true }))

vi.mock('@/lib/platform', async (original) => ({ ...(await original<typeof Platform>()), isIos: () => env.ios }))
vi.mock('@/link/engines', () => ({ LOCAL: 'local', hasLocal: () => true, useEngines: (sel: (s: { active: string }) => unknown) => sel({ active: 'local' }) }))
vi.mock('@/lib/popout', () => ({ isPopout: () => false }))
vi.mock('@/lib/tauri', () => ({ isIsland: () => false, isOverlay: () => false }))
vi.mock('@/lib/notify', () => ({ notify: vi.fn() }))
vi.mock('@/lib/rpc', () => ({ useApi: () => ({}) }))
// Discovery offers list_models, not the engine flag (the local mirror answers); the cloud methods as `env.cloud`.
vi.mock('@/rpc/capabilities', () => ({
  useQuerySupport: (methods: string[]) => !methods.includes('config_flag_get'),
  useSupported: () => env.cloud,
}))
// No model connected yet: setup is due.
vi.mock('@/components/agents/queries', () => ({ useModels: () => ({ data: [], isError: false }) }))
vi.mock('@/store/connection', () => ({ useConnection: (sel: (s: { info: { default_model: string } }) => unknown) => sel({ info: { default_model: '' } }) }))
// `vi.mock` factories run hoisted above the file's imports, so they import what they render inside.
vi.mock('@/components/cloud/CloudNodeWizard', async () => {
  const { createElement: h } = await import('react')
  return {
    CloudNodeWizard: ({ entry, onDone, onClose }: { entry: string; onDone: (n: unknown) => void; onClose: () => void }) =>
      h('div', { 'data-cloud-wizard': entry }, [
        h('button', { key: 'phone', 'data-pick': 'phone', onClick: () => onDone(null) }, 'Just this iPhone'),
        h('button', { key: 'cloud', 'data-pick': 'cloud', onClick: () => onDone({ name: 'sky-cloud' }) }, 'Done'),
        h('button', { key: 'close', 'data-pick': 'close', onClick: onClose }, 'Close'),
      ]),
  }
})
vi.mock('./Wizard', async () => {
  const { createElement: h } = await import('react')
  const { useOnboarding: store } = await import('@/lib/onboarding/state')
  return { Wizard: () => (store((s) => s.wizardOpen) ? h('div', { 'data-setup': store.getState().step }) : null) }
})
vi.mock('./LinkPhoneDialog', () => ({ LinkPhoneDialog: () => null }))
vi.mock('./TourRunner', () => ({ TourRunner: () => null }))

let root: Root | null = null
let host: HTMLElement

beforeEach(() => {
  env.ios = true
  env.cloud = true
  localStorage.clear()
  useOnboarding.setState({ wizardOpen: false, whereOpen: false, runsOn: null, completed: false, step: 'welcome' })
})
afterEach(() => {
  act(() => root?.unmount())
  root = null
  host?.remove()
})

/** A fresh launch: the host mounts with this run's session state cleared, the stored state kept. */
function launch() {
  act(() => root?.unmount())
  useOnboarding.setState({ wizardOpen: false, whereOpen: false })
  host = document.body.appendChild(document.createElement('div'))
  root = createRoot(host)
  act(() => root!.render(createElement(QueryClientProvider, { client: new QueryClient() }, createElement(OnboardingHost))))
}
const where = () => host.querySelector('[data-cloud-wizard="first-run"]')
const setup = () => host.querySelector('[data-setup]')?.getAttribute('data-setup') ?? null
const pick = (p: 'phone' | 'cloud' | 'close') => act(() => host.querySelector<HTMLButtonElement>(`[data-pick="${p}"]`)!.click())

test('the iPhone first run asks where theuth runs before setup; "Just this iPhone" continues to Welcome and is not asked again', () => {
  launch()
  expect(where()).not.toBeNull()
  expect(setup()).toBeNull()
  pick('phone')
  expect(where()).toBeNull()
  expect(setup()).toBe('welcome')
  expect(JSON.parse(localStorage.getItem(ONBOARDING_KEY)!)).toMatchObject({ runsOn: 'device' })
  launch()
  expect(where()).toBeNull()
  expect(setup()).toBe('welcome')
})

test('a created cloud node is remembered and leaves setup to the wizard\'s own "Connect a model"', () => {
  launch()
  pick('cloud')
  expect(where()).toBeNull()
  expect(setup()).toBeNull()
  expect(useOnboarding.getState().runsOn).toBe('cloud')
  launch()
  expect(where()).toBeNull()
})

test('closing without a pick opens Welcome and asks again next launch', () => {
  launch()
  pick('close')
  expect(setup()).toBe('welcome')
  expect(useOnboarding.getState().runsOn).toBeNull()
  launch()
  expect(where()).not.toBeNull()
})

test('desktop first run is unchanged: straight to Welcome', () => {
  env.ios = false
  launch()
  expect(where()).toBeNull()
  expect(setup()).toBe('welcome')
})

test('an iPhone engine without cloud nodes goes straight to Welcome', () => {
  env.cloud = false
  launch()
  expect(where()).toBeNull()
  expect(setup()).toBe('welcome')
})
