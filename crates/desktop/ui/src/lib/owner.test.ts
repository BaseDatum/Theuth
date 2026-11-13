import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { withOwner } from '@/lib/rpc'
import { RpcError } from '@/rpc/client'
import type { SessionSummary } from "@/rpc/bindings/method.agent.output"
import { scope } from '@/store/engine-scope'
import { notOwner, runsElsewhere } from './front-door'
import { engineLabel, lastSeenText, onOwnerChanged, onPeerChange, ownerMidTurnText, ownerOfflineText, ownedElsewhere, ownerUnreachable, remoteAmong, sessionDraining, useOwnerOffline, withdrawRelayed } from './owner'

const KEY = 'abcdef0123456789'.repeat(4)

test('engine label prefers the mesh name, falls back to the short id', () => {
  expect(engineLabel('')).toBe('?')
  expect(engineLabel(KEY)).toBe('abcdef01')
  expect(engineLabel(KEY, [{ id: KEY, name: 'desk' }])).toBe('desk')
  expect(engineLabel(KEY, [{ id: 'other', name: 'desk' }])).toBe('abcdef01')
})

test('OwnerUnreachable is read from the refusal data in both forms, nothing else', () => {
  expect(ownerUnreachable(new Error('x'))).toBeNull()
  expect(ownerUnreachable(new RpcError(-32000, 'owner unreachable'))).toBeNull()
  expect(ownerUnreachable(new RpcError(-32000, 'x', { other: 1 }))).toBeNull()
  expect(ownerUnreachable(new RpcError(-32000, 'x', { OwnerUnreachable: { owner: KEY, last_seen: null } }))).toEqual({
    owner: KEY,
    lastSeen: null,
  })
  expect(ownerUnreachable(new RpcError(-32000, 'x', { kind: 'OwnerUnreachable', owner: KEY, last_seen: 1700000000000 }))).toEqual({
    owner: KEY,
    lastSeen: 1700000000000,
  })
})

test('last seen renders as an age or never; the sentence names the engine twice', () => {
  const now = Date.parse('2026-09-25T12:00:00Z')
  expect(lastSeenText(null)).toBe('never')
  expect(lastSeenText('2026-09-25T11:57:00Z', now)).toBe('3m ago')
  expect(lastSeenText(now - 5000, now)).toBe('5s ago')
  expect(lastSeenText('garbage', now)).toBe('garbage')
  expect(ownerOfflineText('desk', '3m ago')).toBe(
    'Owned by desk, last seen 3m ago. Continue here now? (turns not yet shipped from desk stay there)',
  )
})

test('139 D16: SessionDraining (-32012) is read from the refusal data; a NotHub (-32011) is not one', () => {
  expect(sessionDraining(new Error('x'))).toBeNull()
  expect(sessionDraining(new RpcError(-32011, 'another engine holds the hub lease', { NotHub: { hub: KEY } }))).toBeNull()
  expect(sessionDraining(new RpcError(-32012, 'x', { SessionDraining: { session: 's1', reason: 'release', next_owner: KEY } }))).toEqual({
    session: 's1',
    reason: 'release',
    nextOwner: KEY,
  })
  expect(sessionDraining(new RpcError(-32012, 'x', { SessionDraining: { session: 's1', reason: 'shutdown' } }))).toEqual({
    session: 's1',
    reason: 'shutdown',
    nextOwner: null,
  })
  expect(ownerMidTurnText('desk')).toBe(
    'desk was mid-turn. Continue here and resume — desk may still be working; whatever it did after the last synced step will be discarded.',
  )
})

test('withOwner patches only the named row and leaves a foreign list alone', () => {
  const row = { id: 'a', owner: 'x', epoch: 1, here: false } as SessionSummary
  const rows = [row, { ...row, id: 'b' }]
  expect(withOwner(undefined, { session: 'a', owner: 'me', epoch: 2, here: true })).toBeUndefined()
  expect(withOwner(rows, { session: 'zz', owner: 'me', epoch: 2, here: true })).toBe(rows)
  const next = withOwner(rows, { session: 'a', owner: 'me', epoch: 2, here: true })!
  expect(next[0]).toMatchObject({ id: 'a', owner: 'me', epoch: 2, here: true })
  expect(next[1]).toBe(rows[1])
})

const rows = (...r: [id: string, owner: string, here: boolean][]) => r.map(([id, owner, here]) => ({ id, owner, here }) as SessionSummary)
const ap = (id: string, session: string) => ({ type: 'ApprovalRequested' as const, id, session, tool: 'shell', args: {}, risk: 'mutating' as const })
const ask = (id: string, session: string) => ({ type: 'AskRequested' as const, id, session, questions: [] })

test('183 R1: withdrawRelayed drops a session\'s approvals and asks and leaves the rest', () => {
  const st = scope('test-withdraw')
  st.approvals.getState().replace([ap('a1', 'r'), ap('a2', 'other')])
  st.asks.getState().replace([ask('k1', 'r')])
  withdrawRelayed(st, new Set(['r']))
  expect(st.approvals.getState().pending.map((p) => p.id)).toEqual(['a2'])
  expect(st.asks.getState().pending).toEqual([])
  // Not remembered as resolved: the owner's next Watch may send the card again.
  st.approvals.getState().add(ap('a1', 'r'))
  expect(st.approvals.getState().pending.map((p) => p.id)).toEqual(['a2', 'a1'])
})

test('183 §5.6: the first landing of a chat created elsewhere (epoch 1) keeps its relayed cards; a move (epoch > 1) withdraws them', () => {
  const st = scope('test-landing')
  st.approvals.getState().replace([ap('a1', 'r')])
  st.asks.getState().replace([ask('k1', 'r')])
  onOwnerChanged(st, { session: 'r', epoch: 1 })
  expect(st.approvals.getState().pending.map((p) => p.id)).toEqual(['a1'])
  expect(st.asks.getState().pending.map((p) => p.id)).toEqual(['k1'])
  onOwnerChanged(st, { session: 'r', epoch: 2 })
  expect(st.approvals.getState().pending).toEqual([])
  expect(st.asks.getState().pending).toEqual([])
})

// The node environment has no `window`: the settle request goes out on a stand-in.
const bus = new EventTarget()
beforeEach(() => void vi.stubGlobal('window', bus))
afterEach(() => void vi.unstubAllGlobals())

test('183 R1: a peer disconnect withdraws its remote sessions\' cards and ends follow state; a connect clears their parked calls', () => {
  const st = scope('test-peer')
  const list = rows(['r', 'p', false], ['mine', 'p', true], ['elsewhere', 'q', false])
  st.approvals.getState().replace([ap('a1', 'r'), ap('a2', 'mine'), ap('a3', 'elsewhere')])
  st.sessions.getState().applyEvents([{ type: 'FollowState', session: 'r', live: true }])
  expect(st.sessions.getState().sessions.r?.following).toBe(true)
  const settles: string[] = []
  bus.addEventListener('theuth:reload-session', (ev) => ev instanceof CustomEvent && settles.push(`${ev.detail.session}:${ev.detail.settled}`), { once: true })
  onPeerChange(st, list, { type: 'disconnected', peer: 'p' })
  // R2: the open reply may never see its TurnFinished; the chat is asked to settle from the rows.
  expect(settles).toEqual(['r:true'])
  expect(st.approvals.getState().pending.map((p) => p.id)).toEqual(['a2', 'a3'])
  expect(st.sessions.getState().sessions.r?.following).toBe(false)
  const park = useOwnerOffline.getState().park
  park('r', { owner: 'p', lastSeen: null })
  park('elsewhere', { owner: 'q', lastSeen: null })
  onPeerChange(st, list, { type: 'connected', peer: 'p' })
  expect(Object.keys(useOwnerOffline.getState().by)).toEqual(['elsewhere'])
})

test('183 §13: NotOwner (-32013) and RunsElsewhere (-32014) are read from their own code and tag only', () => {
  expect(notOwner(new RpcError(-32013, 'x', { NotOwner: { owner: KEY } }))).toEqual({ owner: KEY })
  expect(notOwner(new RpcError(-32014, 'x', { RunsElsewhere: { owner: KEY } }))).toBeNull()
  expect(notOwner(new RpcError(-32013, 'x'))).toBeNull()
  expect(runsElsewhere(new RpcError(-32014, 'x', { RunsElsewhere: { owner: KEY } }))).toEqual({ owner: KEY })
  expect(runsElsewhere(new Error('x'))).toBeNull()
})

test('183 R1: leaving the focus set withdraws only remote sessions\' cards', () => {
  const st = scope('test-unfocus')
  const list = rows(['r', 'p', false], ['mine', 'p', true])
  st.approvals.getState().replace([ap('a1', 'r'), ap('a2', 'mine')])
  st.asks.getState().replace([ask('k1', 'r')])
  withdrawRelayed(st, remoteAmong(list, ['r', 'mine', 'unknown']))
  expect(st.approvals.getState().pending.map((p) => p.id)).toEqual(['a2'])
  expect(st.asks.getState().pending).toEqual([])
})

test('183 §5.1: a TurnFinished settles only a session known to be owned elsewhere', () => {
  const list = rows(['remote', 'p', false], ['local', 'me', true])
  expect(ownedElsewhere(list, 'remote')).toBe(true)
  expect(ownedElsewhere(list, 'local')).toBe(false)
  expect(ownedElsewhere(list, 'unknown')).toBe(false)
  expect(ownedElsewhere(undefined, 'remote')).toBe(false)
})
