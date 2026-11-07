import { expect, test } from 'vitest'
import type { MeshStatus } from '@/rpc/bindings/MeshStatus'
import { moveBlocked, moveTargets } from './SessionMenu'

const now = Date.UTC(2026, 10, 7, 17, 20)
const peer = (id: string, name: string, reachable: boolean, last_seen_ms: number) =>
  ({ id, name, addrs: [], reachable, last_seen_ms, hub_pref: 'eligible' }) as unknown as MeshStatus['peers'][number]
const mesh = {
  self_id: 'me',
  peers: [peer('x1', 'xeus', true, now), peer('me', 'this mac', true, now), peer('l1', "Sky's laptop", false, now - 3 * 3_600_000), peer('n1', '', false, 0)],
}

test('181 D1: Move to ▸ lists every paired engine but this one; offline is disabled with its age; running jobs stay here', () => {
  expect(moveTargets(mesh, 0, now)).toEqual([
    { id: 'x1', name: 'xeus', online: true, detail: '' },
    { id: 'l1', name: "Sky's laptop", online: false, detail: 'offline 3h' },
    { id: 'n1', name: 'n1', online: false, detail: 'offline' },
  ])
  expect(moveTargets(mesh, 1, now)[0]?.detail).toBe('1 running job stays here')
  expect(moveTargets(mesh, 2, now).map((t) => t.detail)).toEqual(['2 running jobs stay here', 'offline 3h', 'offline'])
  // D6: no paired engines, no submenu.
  expect(moveTargets({ self_id: 'me', peers: [] }, 3, now)).toEqual([])
  expect(moveTargets(undefined, 0, now)).toEqual([])
})

test('181 fix round: a paused agent’s tree, a desk and a harness session say why they cannot move', () => {
  const rows = [
    { id: 'desk', parent: null, agent: null, desk: 'ops' },
    { id: 'child', parent: 'desk', agent: null },
    { id: 'grand', parent: 'child', agent: null },
    { id: 'acp', parent: null, agent: 'omp' },
    { id: 'chat', parent: null, agent: null },
  ]
  const paused = [{ status: 'paused' as const, desk: 'desk', file: { name: 'ops' } }] as never[]
  const row = (id: string) => rows.find((r) => r.id === id)
  for (const id of ['desk', 'child', 'grand']) expect(moveBlocked(row(id), rows, paused)).toBe('ops is paused — un-pause it before moving')
  expect(moveBlocked(row('desk'), rows, [])).toBe("ops's desk stays on the engine that runs its agent")
  expect(moveBlocked(row('child'), rows, [])).toBeNull()
  expect(moveBlocked(row('acp'), rows, paused)).toBe('runs the omp harness as a process here, so it cannot move')
  expect(moveBlocked(row('chat'), rows, paused)).toBeNull()
  expect(moveBlocked(undefined, rows, paused)).toBeNull()
})