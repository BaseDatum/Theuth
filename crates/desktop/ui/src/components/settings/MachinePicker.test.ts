import './test-dom'
import { QueryClient } from '@tanstack/react-query'
import { act, createElement as h } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, expect, test } from 'vitest'
import { MachinePicker } from '@/components/settings/MachinePicker'
import { type SettingsMachines, SettingsMachinesProvider, usePickedMachine } from '@/components/settings/machine-target'
import type { Api } from '@/rpc/bindings/client'
import type { Machine } from '@/lib/mesh-machines'

const machine = (id: string, name: string, m: Partial<Machine> = {}): Machine => ({ id, name, kind: 'peer', connected: false, client: 'hub-row', forwarded: true, hub: false, online: true, sealed: false, path: 'direct', ...m })
const MACHINES = [
  machine('phone', 'This iPhone', { kind: 'local', path: null }),
  machine('mbp', '333 · MacBook Pro', { kind: 'hub', hub: true, connected: true, path: null }),
  machine('p50', 'P50 · ThinkPad'),
  machine('nyc', 'cloud-nyc1', { kind: 'cloud', sealed: true, path: 'relay' }),
  machine('xeus', 'xeus', { online: false, path: null }),
]

afterEach(() => {
  document.body.innerHTML = ''
  usePickedMachine.setState({ picked: null })
})

async function mount(phone: boolean, machines = MACHINES) {
  const connected = machines.find((m) => m.connected) ?? null
  const settings: SettingsMachines = { machines, target: connected, connected, api: {} as Api, local: null, meshClient: 'hub-row', qc: new QueryClient(), tab: null, open: () => {} }
  const host = document.createElement('div')
  document.body.append(host)
  await act(async () => createRoot(host).render(h(SettingsMachinesProvider, { value: settings }, h(MachinePicker, { phone }))))
  return host
}

test('227: on a phone the picker is a header row that opens a menu: every machine with its presence, offline ones disabled; a pick sets the window\'s machine', async () => {
  const host = await mount(true)
  const trigger = host.querySelector<HTMLButtonElement>('[data-machine-picker]')!
  expect(trigger.dataset.machinePicker).toBe('phone')
  expect(trigger.textContent).toBe('333 · MacBook Prohub')
  await act(async () => {
    trigger.focus()
    trigger.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }))
  })
  const items = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')]
  expect(items.map((i) => [i.textContent, i.hasAttribute('data-disabled')])).toEqual([
    ['This iPhonelocal', false],
    ['333 · MacBook Prohub ✓', false],
    ['P50 · ThinkPaddirect', false],
    ['cloud-nyc1vault locked', false],
    ['xeusoffline', true],
  ])
  expect([...document.querySelectorAll('[data-presence]')].slice(1).map((d) => d.getAttribute('data-presence'))).toEqual(['online', 'online', 'online', 'sealed', 'offline'])
  await act(async () => items[2]!.click())
  expect(usePickedMachine.getState().picked).toBe('p50')
})

test('227: the sidebar variant on iPad/desktop; one machine (or an engine without engine_call): no picker', async () => {
  const host = await mount(false)
  expect(host.querySelector<HTMLElement>('[data-machine-picker]')?.dataset.machinePicker).toBe('sidebar')
  document.body.innerHTML = ''
  expect((await mount(false, [MACHINES[1]!])).querySelector('[data-machine-picker]')).toBeNull()
})
