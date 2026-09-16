// Switcher tree order, then auto-pair (32 §8.2) against a fake parent `Api` with
// the pairing handshake replaced by a function that records the row `pair()` would add.
import { beforeEach, expect, test, vi } from 'vitest'
import type { Node } from '@/rpc/bindings/Node'
import { RpcError } from '@/rpc/client'
import { type Harness, harnessTree, useHarnesses } from './harnesses'
import { type NodesApi, type PairFn, adopt, autoPair, onNodeEvent } from './nodes'

const PARENT = 'parent-id'

const row = (id: string, order: number, parent?: string): Harness => ({
  id,
  name: id,
  relayHost: 'relay.test',
  secure: true,
  pairedAt: 1,
  lastSeen: 1,
  order,
  ...(parent ? { parent } : {}),
})

const node = (id: string, online: boolean): Node => ({
  id,
  name: `node ${id}`,
  relay: 'wss://relay.test:8787',
  platform: 'linux',
  version: '0.4.1',
  paired_at_ms: 1n,
  last_seen_ms: 0n,
  online,
  revoked: false,
  children: 0,
})

/** `invite_node` hands out `k:<id>`; `pairFn` turns that into the row `pair()` would add. */
function fakeApi(nodes: Node[] | RpcError) {
  const invite = vi.fn((id: string) => Promise.resolve({ key: `k:${id}`, expires_at_ms: 0n }))
  const api: NodesApi = {
    list_nodes: () => (nodes instanceof RpcError ? Promise.reject(nodes) : Promise.resolve(nodes)),
    invite_node: invite,
  }
  return { api, invite }
}

const pairFn: PairFn = (key) => {
  const id = key.slice(2)
  const h = row(id, 99)
  h.pairedAt = Date.now()
  useHarnesses.getState().add(h)
  return Promise.resolve(h)
}

beforeEach(() => {
  useHarnesses.setState({ harnesses: [row(PARENT, 0)], presence: {}, pairError: {}, nodes: {}, active: PARENT })
})

test('harnessTree: children right after their parent in order, orphans flat, one indent', () => {
  const rows = harnessTree([row('c2', 5, 'p'), row('p', 1), row('orphan', 2, 'gone'), row('c1', 3, 'p'), row('q', 0)])
  expect(rows.map((r) => [r.harness.id, r.depth])).toEqual([
    ['q', 0],
    ['p', 0],
    ['c1', 1],
    ['c2', 1],
    ['orphan', 0],
  ])
  // `add` keeps the store in tree order
  const s = useHarnesses.getState()
  s.add(row('child', 7, PARENT))
  s.add(row('other', 3))
  expect(useHarnesses.getState().harnesses.map((h) => h.id)).toEqual([PARENT, 'child', 'other'])
})

test('autoPair: an online unknown node is invited and paired with `parent` set', async () => {
  const { api, invite } = fakeApi([node('n1', true)])
  await autoPair(PARENT, api, pairFn)
  expect(invite).toHaveBeenCalledWith('n1')
  const h = useHarnesses.getState().harnesses.find((x) => x.id === 'n1')
  expect(h).toMatchObject({ parent: PARENT })
  expect(h!.pairedAt).toBeGreaterThan(0)
  expect(useHarnesses.getState().nodes[PARENT]).toBe(true)
})

test('autoPair: an offline node gets a placeholder; NodeOnline{true} runs the invite', async () => {
  const { api, invite } = fakeApi([node('n2', false)])
  await autoPair(PARENT, api, pairFn)
  expect(invite).not.toHaveBeenCalled()
  expect(useHarnesses.getState().harnesses.find((x) => x.id === 'n2')).toMatchObject({
    parent: PARENT,
    pairedAt: 0,
    relayHost: 'relay.test:8787',
    secure: true,
  })
  expect(useHarnesses.getState().presence.n2).toBe('offline')

  onNodeEvent(PARENT, api, { type: 'NodeOnline', id: 'n2', online: false }, pairFn)
  expect(invite).not.toHaveBeenCalled()
  onNodeEvent(PARENT, api, { type: 'NodeOnline', id: 'n2', online: true }, pairFn)
  await vi.waitFor(() => expect(useHarnesses.getState().harnesses.find((x) => x.id === 'n2')?.pairedAt).toBeGreaterThan(0))
  expect(invite).toHaveBeenCalledTimes(1)
  expect(useHarnesses.getState().harnesses.find((x) => x.id === 'n2')?.parent).toBe(PARENT)
})

test('autoPair: method-not-found adds nothing and marks the harness as without nodes', async () => {
  const { api, invite } = fakeApi(new RpcError(-32601, 'Method not found'))
  await autoPair(PARENT, api, pairFn)
  expect(invite).not.toHaveBeenCalled()
  expect(useHarnesses.getState().harnesses.map((h) => h.id)).toEqual([PARENT])
  expect(useHarnesses.getState().nodes[PARENT]).toBe(false)
})

test('autoPair: a node paired earlier by scan only gets `parent`; removal clears it', async () => {
  const before = row('n3', 4)
  useHarnesses.getState().add(before)
  const { api, invite } = fakeApi([node('n3', true)])
  await autoPair(PARENT, api, pairFn)
  expect(invite).not.toHaveBeenCalled()
  expect(useHarnesses.getState().harnesses.find((x) => x.id === 'n3')).toMatchObject({ ...before, parent: PARENT })

  onNodeEvent(PARENT, api, { type: 'NodeRemoved', id: 'n3' }, pairFn)
  expect(useHarnesses.getState().harnesses.find((x) => x.id === 'n3')?.parent).toBeUndefined()
})

test('adopt: a failed pairing leaves a row with pairError after the retry', async () => {
  vi.useFakeTimers()
  try {
    const { api, invite } = fakeApi([])
    const failing: PairFn = () => Promise.reject(new Error('expired or used key'))
    const done = adopt(PARENT, api, node('n4', true), failing)
    await vi.advanceTimersByTimeAsync(5_000)
    await done
    expect(invite).toHaveBeenCalledTimes(2)
    expect(useHarnesses.getState().pairError.n4).toBe('expired or used key')
    expect(useHarnesses.getState().harnesses.find((x) => x.id === 'n4')).toMatchObject({ parent: PARENT, pairedAt: 0 })
  } finally {
    vi.useRealTimers()
  }
})
