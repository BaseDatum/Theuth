import './test-dom'
import { act, createElement, type ReactElement } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, expect, test } from 'vitest'
import { NotHereTag, ScopeHeader, ScopeTag } from './ScopeTag'
import type { ScopeNames, SettingsScope } from './scope'

const names: ScopeNames = { engine: 'P50', hub: '333', device: 'iPad' }
let host: HTMLDivElement | null = null

function mount(node: ReactElement): HTMLElement {
  host = document.createElement('div')
  document.body.append(host)
  const root = createRoot(host)
  act(() => root.render(node))
  return host
}

afterEach(() => {
  host?.remove()
  host = null
})

test.each<[SettingsScope, ScopeNames, string, string]>([
  ['machine', names, 'machine', 'Saved on P50. Other machines keep their own.'],
  ['everywhere', names, 'everywhere', 'Shared by every machine in your mesh.'],
  ['hub', names, 'hub only', 'Runs on the hub (333).'],
  ['hub', { ...names, hub: null }, 'hub only', 'Runs on the hub.'],
  ['device', names, 'this device', 'Only on this iPad.'],
  ['machine', { ...names, engine: null }, 'machine', 'Saved on this machine. Other machines keep their own.'],
])('the %s tag reads its label and tooltip', (scope, given, label, tooltip) => {
  const tag = mount(createElement(ScopeTag, { scope, names: given })).querySelector(`[data-scope="${scope}"]`)
  expect(tag?.textContent).toBe(label)
  expect(tag?.getAttribute('title')).toBe(tooltip)
})

test('a tab the engine cannot run says so in its header and keeps its scope tag; one that can shows the tag alone', () => {
  const na = mount(createElement(ScopeHeader, { scope: 'machine', names, missing: ['mic', 'user'] }))
  expect(na.querySelector('[data-scope="machine"]')).not.toBeNull()
  expect(na.querySelector('[role="note"]')?.textContent).toBe('Not on this machine. P50 has no microphone or signed-in user, so this does nothing there.')
  host?.remove()
  const ok = mount(createElement(ScopeHeader, { scope: 'machine', names, missing: null }))
  expect(ok.querySelector('[role="note"]')).toBeNull()
})

test('the nav mark of an n/a tab is `n/a` with the reason as its tooltip', () => {
  const mark = mount(createElement(NotHereTag, { missing: ['display'], engine: 'cloud-nyc1' })).querySelector('[data-not-here]')
  expect(mark?.textContent).toBe('n/a')
  expect(mark?.getAttribute('title')).toBe('Not on this machine. cloud-nyc1 has no display, so this does nothing there.')
})
