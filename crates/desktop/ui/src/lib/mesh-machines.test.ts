import { expect, test } from 'vitest'
import type { CloudList } from '@/components/cloud/types'
import type { EngineCapsResult } from '@/rpc/bindings/method.agent.output'
import { type Machine, type MachinesInput, meshMachines } from '@/lib/mesh-machines'
import { LOCAL, LOCAL_NAME } from '@/link/engines'
import type { MeshStatus, Peer } from '@/rpc/bindings/method.nodes.output'

const peer = (id: string, name: string, p: Partial<Peer> = {}): Peer => ({ id, name, addrs: [], hub_pref: 'auto', last_seen_ms: 0, path: 'lan', reachable: true, rtt_ms: 3, ...p }) as Peer

const mesh = (holder: string | null, peers: Peer[], self = { id: 'mbp', name: '333 · MacBook Pro' }): MeshStatus =>
  ({ self_id: self.id, self_name: self.name, mesh_name: 'home', lease: { holder, reason: '', since_ms: 0 }, peers }) as unknown as MeshStatus

const cloud = (nodes: { name: string; peer_id: string; sealed?: boolean }[]): CloudList => ({ errors: [], nodes: nodes as unknown as CloudList['nodes'] })

/** Viewing the hub's mesh over the link (`mesh-row` active), no local engine unless given. */
const viewHub = (m: MeshStatus | undefined, extra: Partial<MachinesInput> = {}) => meshMachines({ mesh: m, meshClient: 'mesh-row', active: 'mesh-row', local: null, ...extra })

const rows = (ms: Machine[]) => ms.map((m) => `${m.name}:${m.kind}${m.connected ? '*' : ''}`)

test('227: the hub first, then peers by name, cloud nodes last; the active engine is the default', () => {
  const ms = viewHub(mesh('mbp', [peer('zed', 'zed'), peer('c1', 'cloud-nyc1'), peer('p50', 'P50 · ThinkPad')]), { cloud: cloud([{ name: 'cloud-nyc1', peer_id: 'c1' }]) })
  expect(rows(ms)).toEqual(['333 · MacBook Pro:hub*', 'P50 · ThinkPad:peer', 'zed:peer', 'cloud-nyc1:cloud'])
  expect(ms.map((m) => [m.client, m.forwarded])).toEqual([['mesh-row', false], ['mesh-row', true], ['mesh-row', true], ['mesh-row', true]])
})

test('227 (a): this device\'s engine is a member of the mesh shown: listed once, first, as LOCAL_NAME, through its own client', () => {
  const m = mesh('mbp', [peer('phone-eng', 'iPhone engine'), peer('p50', 'P50')])
  const ms = viewHub(m, { local: { online: true, meshId: 'phone-eng', meshName: 'home' } })
  expect(rows(ms)).toEqual([`${LOCAL_NAME}:local`, '333 · MacBook Pro:hub*', 'P50:peer'])
  expect(ms[0]).toMatchObject({ id: 'phone-eng', client: LOCAL, forwarded: false, note: undefined })
  expect(ms.filter((x) => x.id === 'phone-eng')).toHaveLength(1)
})

test('227 (b): not a member: still first, through its own client, noted "not in this mesh" or "in <its mesh>"', () => {
  const m = mesh('mbp', [peer('p50', 'P50')])
  const alone = viewHub(m, { local: { online: true, meshId: 'phone-eng', meshName: null } })
  expect(rows(alone)).toEqual([`${LOCAL_NAME}:local`, '333 · MacBook Pro:hub*', 'P50:peer'])
  expect(alone[0]).toMatchObject({ id: 'phone-eng', client: LOCAL, forwarded: false, note: 'not in this mesh' })
  expect(viewHub(m, { local: { online: true, meshId: 'phone-eng', meshName: 'office' } })[0]!.note).toBe('in office')
  // Before its engine answered: listed under LOCAL, never as a member.
  expect(viewHub(m, { local: { online: false, meshId: null, meshName: null } })[0]).toMatchObject({ id: LOCAL, online: false })
})

test('227: this device\'s engine active and in a mesh: its own mesh is shown through its client, members forwarded through it', () => {
  const m = mesh('p50', [peer('p50', 'P50'), peer('mbp', '333')], { id: 'mac-eng', name: 'mac' })
  const ms = meshMachines({ mesh: m, meshClient: LOCAL, active: LOCAL, local: { online: true, meshId: 'mac-eng', meshName: 'home' } })
  expect(rows(ms)).toEqual([`${LOCAL_NAME}:local*`, 'P50:hub', '333:peer'])
  expect(ms.map((x) => [x.id, x.client, x.forwarded])).toEqual([['mac-eng', LOCAL, false], ['p50', LOCAL, true], ['mbp', LOCAL, true]])
})

test('227: this device\'s engine active, viewing another mesh: LOCAL is the default, the mesh\'s engine reached by its own client', () => {
  const ms = meshMachines({ mesh: mesh('mbp', [peer('p50', 'P50')]), meshClient: 'mesh-row', active: LOCAL, local: { online: true, meshId: 'phone-eng', meshName: null } })
  expect(rows(ms)).toEqual([`${LOCAL_NAME}:local*`, '333 · MacBook Pro:hub', 'P50:peer'])
  expect(ms[1]).toMatchObject({ client: 'mesh-row', forwarded: false, connected: false })
})

test('227: presence and paths; a mesh client without engine_call lists only the engines with a client of their own', () => {
  const m = mesh(null, [peer('a', 'a', { path: 'lan' }), peer('b', 'b', { path: 'relay' }), peer('c', 'c', { reachable: false }), peer('d', 'd', { path: 'offline' })])
  const by = Object.fromEntries(viewHub(m).map((x) => [x.id, [x.online, x.path]]))
  expect(by).toEqual({ mbp: [true, null], a: [true, 'direct'], b: [true, 'relay'], c: [false, null], d: [false, null] })
  expect(viewHub(m, { forwarding: false, local: { online: true, meshId: 'x', meshName: null } }).map((x) => x.id)).toEqual(['x', 'mbp'])
})

test('227: sealed from the cloud list or from an engine_caps that answered "sealed"; caps attached by id', () => {
  const caps = { p50: { engine: 'p50', name: 'P50', platform: 'linux', display: false, mic: false, interactive_user: false, headless: true, hub: false } satisfies EngineCapsResult }
  const ms = viewHub(mesh(null, [peer('c1', 'cloud-nyc1'), peer('p50', 'P50'), peer('x', 'xeus')]), { cloud: cloud([{ name: 'cloud-nyc1', peer_id: 'c1', sealed: true }]), caps, sealed: new Set(['x']) })
  const by = Object.fromEntries(ms.map((m) => [m.id, m]))
  expect([by.c1!.sealed, by.x!.sealed, by.p50!.sealed]).toEqual([true, true, false])
  expect(by.p50!.caps).toBe(caps.p50)
  expect(viewHub(undefined)).toEqual([])
})
