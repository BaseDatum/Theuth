import './test-dom'
import { act, createElement as h } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, expect, test, vi } from 'vitest'
import type { DeviceInfo } from '@/rpc/bindings/method.nodes.output'
import { DeviceList } from './DeviceList'
import { INACTIVE_MS, deleteDevice, deleteDevices, groupDevices } from './devices'

const NOW = 1_000 * INACTIVE_MS
const DAY = 24 * 60 * 60 * 1000
const dev = (id: string, over: Partial<DeviceInfo> = {}): DeviceInfo => ({ id, name: id, platform: 'ios', paired_ms: NOW - 200 * DAY, last_seen_ms: NOW - DAY, revoked: false, ...over })

const phone = dev('phone')
const ipad = dev('ipad', { last_seen_ms: NOW - 91 * DAY })
const never = dev('never', { last_seen_ms: 0 })
const fresh = dev('fresh', { last_seen_ms: 0, paired_ms: NOW - DAY })
const live = dev('live', { last_seen_ms: NOW - 400 * DAY })
const safari = dev('safari', { platform: 'web', revoked: true })
const ALL = [phone, ipad, never, fresh, live, safari]

test('devices group into active, inactive after 90 days unseen, and revoked', () => {
  const g = groupDevices(ALL, new Set(['live']), NOW)
  expect(g.active.map((d) => d.id)).toEqual(['phone', 'fresh', 'live'])
  // Never seen counts from pairing; connected now is active however old the last close.
  expect(g.inactive.map((d) => d.id)).toEqual(['ipad', 'never'])
  expect(g.revoked.map((d) => d.id)).toEqual(['safari'])
  expect(groupDevices([dev('edge', { last_seen_ms: NOW - INACTIVE_MS })], new Set(), NOW).inactive).toHaveLength(1)
})

test('rows show platform, last seen and status; revoked rows stay collapsed', () => {
  const html = renderToStaticMarkup(
    h(DeviceList, { devices: ALL, online: new Set(['live']), now: NOW, onRename() {}, onRevoke() {}, onDelete() {}, onDeleteAll() {} }),
  )
  expect(html).toContain('iOS · seen 24h ago · offline')
  expect(html).toContain('iOS · seen now · connected now')
  expect(html).toContain('iOS · never seen · offline')
  expect(html).toContain('Inactive · not seen for 90 days (2)')
  expect(html).toContain('Delete all inactive')
  expect(html).toContain('Revoked (1)')
  expect(html).not.toContain('safari')
})

test('an empty registry says so', () => {
  const html = renderToStaticMarkup(h(DeviceList, { devices: [], online: new Set<string>(), now: NOW, onRename() {}, onRevoke() {}, onDelete() {}, onDeleteAll() {} }))
  expect(html).toContain('No device paired with this mesh yet')
})

test('Delete revokes an active device first, a revoked one is deleted at once', async () => {
  const calls: string[] = []
  const revoke = async (id: string) => void calls.push(id)
  await deleteDevice(revoke, phone)
  expect(calls).toEqual(['phone', 'phone'])
  calls.length = 0
  await deleteDevice(revoke, safari)
  expect(calls).toEqual(['safari'])
  calls.length = 0
  await deleteDevices(revoke, [ipad, safari])
  expect(calls).toEqual(['ipad', 'ipad', 'safari'])
})

let root: Root | null = null
afterEach(() => {
  act(() => root?.unmount())
  root = null
  document.body.innerHTML = ''
})

const click = (el: Element | null | undefined) => {
  expect(el).toBeTruthy()
  act(() => (el as HTMLElement).click())
}
const button = (scope: ParentNode, text: string) => [...scope.querySelectorAll('button')].find((b) => b.textContent?.trim() === text)

test('confirming Delete on a row, and Delete all inactive, hands over the right devices', () => {
  const onDelete = vi.fn()
  const onDeleteAll = vi.fn()
  const host = document.body.appendChild(document.createElement('div'))
  root = createRoot(host)
  act(() => root!.render(h(DeviceList, { devices: ALL, online: new Set<string>(), now: NOW, onRename() {}, onRevoke() {}, onDelete, onDeleteAll })))

  // Expand Revoked: the Safari row shows, with Delete and no Revoke.
  click(button(host, 'Revoked (1)'))
  const revokedGroup = host.querySelector('[data-device-group="revoked"]')!
  expect(revokedGroup.textContent).toContain('safari')
  expect(button(revokedGroup, 'Revoke')).toBeUndefined()
  click(button(revokedGroup, 'Delete'))
  click(button(document.querySelector('[role="alertdialog"]')!, 'Delete'))
  expect(onDelete).toHaveBeenCalledWith(safari)

  click(button(host, 'Delete all inactive'))
  click(button(document.querySelector('[role="alertdialog"]')!, 'Delete all inactive'))
  expect(onDeleteAll).toHaveBeenCalledWith([ipad, never, live])
})
