import { expect, test } from 'vitest'
import type { CloudList } from '@/components/cloud/types'
import type { WireEngineCaps } from '@/lib/engine-call.types'
import { type Machine, meshMachines } from '@/lib/mesh-machines'
import type { MeshStatus, Peer } from '@/rpc/bindings/method.nodes.output'

const peer = (id: string, name: string, p: Partial<Peer> = {}): Peer => ({ id, name, addrs: [], hub_pref: 'auto', last_seen_ms: 0, path: 'lan', reachable: true, rtt_ms: 3, ...p }) as Peer

const mesh = (holder: string | null, peers: Peer[], self = { id: 'mac', name: '333 · MacBook Pro' }): MeshStatus =>
  ({ self_id: self.id, self_name: self.name, lease: { holder, reason: '', since_ms: 0 }, peers }) as unknown as MeshStatus

const cloud = (nodes: { name: string; peer_id: string; sealed?: boolean }[]): CloudList => ({ errors: [], nodes: nodes as unknown as CloudList['nodes'] })

const order = (ms: Machine[]) => ms.map((m) => `${m.name}:${m.kind}`)

test('227: this device first, then the hub, then peers by name, cloud nodes last', () => {
  const ms = meshMachines({
    mesh: mesh('hub', [peer('zed', 'zed'), peer('c1', 'cloud-nyc1'), peer('hub', '333 · MacBook Pro'), peer('p50', 'P50 · ThinkPad')], { id: 'phone', name: 'phone engine' }),
    cloud: cloud([{ name: 'cloud-nyc1', peer_id: 'c1' }]),
    local: true,
    localName: 'This iPhone',
  })
  expect(order(ms)).toEqual(['This iPhone:local', '333 · MacBook Pro:hub', 'P50 · ThinkPad:peer', 'zed:peer', 'cloud-nyc1:cloud'])
  expect(ms[0]).toMatchObject({ id: 'phone', connected: true, online: true, path: null })
})

test('227: connected over the link to the hub, the hub is the connected machine (not local) and keeps its hub kind', () => {
  const ms = meshMachines({ mesh: mesh('mac', [peer('p50', 'P50')]), local: false })
  expect(ms[0]).toMatchObject({ id: 'mac', name: '333 · MacBook Pro', kind: 'hub', hub: true, connected: true })
  // The local daemon that also holds the lease is `local`, flagged hub.
  expect(meshMachines({ mesh: mesh('mac', []), local: true, localName: 'This Mac' })[0]).toMatchObject({ kind: 'local', hub: true, name: 'This Mac' })
})

test('227: presence: an unreachable or offline-path peer is offline with no path; lan is direct, relay is relay', () => {
  const ms = meshMachines({
    mesh: mesh(null, [peer('a', 'a', { path: 'lan' }), peer('b', 'b', { path: 'relay' }), peer('c', 'c', { reachable: false }), peer('d', 'd', { path: 'offline' })]),
    local: false,
  })
  const by = Object.fromEntries(ms.map((m) => [m.id, [m.online, m.path]]))
  expect(by).toEqual({ mac: [true, null], a: [true, 'direct'], b: [true, 'relay'], c: [false, null], d: [false, null] })
})

test('227: sealed from the cloud list or from an engine_caps that answered "sealed"; caps attached by id', () => {
  const caps = { p50: { engine: 'p50', name: 'P50', platform: 'linux', display: false, mic: false, interactive_user: false, headless: true, hub: false } satisfies WireEngineCaps }
  const ms = meshMachines({
    mesh: mesh(null, [peer('c1', 'cloud-nyc1'), peer('p50', 'P50'), peer('x', 'xeus')]),
    cloud: cloud([{ name: 'cloud-nyc1', peer_id: 'c1', sealed: true }]),
    local: false,
    caps,
    sealed: new Set(['x']),
  })
  const by = Object.fromEntries(ms.map((m) => [m.id, m]))
  expect([by.c1!.sealed, by.x!.sealed, by.p50!.sealed]).toEqual([true, true, false])
  expect(by.p50!.caps).toBe(caps.p50)
  expect(meshMachines({ mesh: undefined, local: true })).toEqual([])
})
