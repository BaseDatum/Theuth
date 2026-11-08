import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { parkIfOffline, retryAfterOwnerChange, retryNotOwner, routed, ownerState, useOwnerOffline } from '@/lib/owner'
import { dispatch } from '@/store/sessions'
import { RpcError } from '@/rpc/client'
import { EngineChip } from '@/components/sidebar/SessionItem'
import { OwnerOfflineCard } from './OwnerOfflineCard'

const CLOUD = 'c10d'.repeat(16)
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<object>()), useApi: () => ({}) }))
vi.mock('@/components/tasks/queries', () => ({ useMesh: () => ({ data: { self_id: 'me', peers: [{ id: CLOUD, name: 'cloud-1', reachable: true }] } }) }))

const offline = () => new RpcError(-32010, 'owner unreachable', { OwnerUnreachable: { owner: CLOUD, last_seen: Date.now() - 3 * 3_600_000 } })

test('183 §4.4 (D2): a routed call refused -32010 is parked behind the card; nothing runs until Continue here', async () => {
  let calls = 0
  const set = () => {
    calls++
    return calls === 1 ? Promise.reject(offline()) : Promise.resolve('ran')
  }
  expect(await routed('s1', set)).toBeUndefined()
  const parked = useOwnerOffline.getState().by.s1
  expect(parked).toMatchObject({ owner: CLOUD })
  expect(calls).toBe(1)
  // Continue here takes the session, then runs the very call (the card's `onResend`).
  expect(await parked?.retry?.()).toBe('ran')
  // Any other failure is the caller's; nothing is parked.
  await expect(routed('s2', () => Promise.reject(new RpcError(-32000, 'boom')))).rejects.toThrow('boom')
  expect(useOwnerOffline.getState().by.s2).toBeUndefined()
  useOwnerOffline.getState().clear('s1')
  expect(useOwnerOffline.getState().by.s1).toBeUndefined()
})

test('the card above the composer asks "Continue here?" naming the owner', () => {
  const html = renderToStaticMarkup(createElement(OwnerOfflineCard, { session: 's1', offline: { owner: CLOUD, lastSeen: Date.now() - 3 * 3_600_000 }, onDone() {}, async onResend() {} }))
  expect(html).toContain('Owned by cloud-1, last seen 3h ago. Continue here now?')
  expect(html).toContain('>Continue here</button>')
  expect(html).toContain('>Wait</button>')
})

test('183 §12.1: the header chip — live with a green dot, offline, or bare before the roster knows the owner', () => {
  const peers = [{ id: CLOUD, reachable: true }]
  expect(ownerState(CLOUD, peers, false, true)).toBe('live')
  // 183 R1: connected is not live — only an up follow stream (`FollowState`) earns the dot.
  expect(ownerState(CLOUD, peers, false)).toBe('connected')
  expect(ownerState(CLOUD, peers, true, true)).toBe('offline')
  expect(ownerState(CLOUD, [{ id: CLOUD, reachable: false }], false)).toBe('offline')
  expect(ownerState(CLOUD, peers, true)).toBe('offline')
  expect(ownerState(CLOUD, [], false)).toBeUndefined()
  const chip = (state?: 'live' | 'connected' | 'offline') => renderToStaticMarkup(createElement(EngineChip, { owner: CLOUD, epoch: 3, prefix: 'on ', state }))
  expect(chip('live')).toMatch(/<span[^>]*bg-success[^>]*><\/span>on cloud-1 · live<\/span>$/)
  expect(chip('offline')).toMatch(/>on cloud-1 · offline<\/span>$/)
  expect(chip('offline')).not.toContain('bg-success')
  expect(chip()).toMatch(/>on cloud-1<\/span>$/)
  expect(chip('connected')).toMatch(/>on cloud-1<\/span>$/)
})

test('183 §4.4 (R1): a flush or a ⋯ command refused -32010 parks itself for the card; any other error is the caller\'s', async () => {
  useOwnerOffline.getState().clear('s-cmd')
  const retry = vi.fn(async () => 'ran')
  expect(parkIfOffline('s-cmd', new Error('boom'), retry)).toBe(false)
  expect(useOwnerOffline.getState().by['s-cmd']).toBeUndefined()
  expect(parkIfOffline('s-cmd', offline(), retry)).toBe(true)
  const parked = useOwnerOffline.getState().by['s-cmd']!
  expect(parked.owner).toBe(CLOUD)
  expect(retry).not.toHaveBeenCalled() // nothing runs until Continue here
  await parked.retry!()
  expect(retry).toHaveBeenCalledTimes(1)
})

test('183 §13 row 2: after NotOwner the send retries once on the next SessionOwnerChanged of that session, never on another\'s', () => {
  vi.useFakeTimers()
  const retry = vi.fn()
  const giveUp = vi.fn()
  retryAfterOwnerChange('s1', retry, giveUp)
  dispatch([{ type: 'SessionOwnerChanged', session: 'other', owner: CLOUD, epoch: 2, here: false }])
  expect(retry).not.toHaveBeenCalled()
  dispatch([{ type: 'SessionOwnerChanged', session: 's1', owner: CLOUD, epoch: 3, here: false }])
  dispatch([{ type: 'SessionOwnerChanged', session: 's1', owner: CLOUD, epoch: 4, here: false }])
  expect(retry).toHaveBeenCalledTimes(1)
  vi.advanceTimersByTime(60_000)
  expect(giveUp).not.toHaveBeenCalled()
  // No change comes: the refusal is shown instead.
  retryAfterOwnerChange('s2', retry, giveUp, 1000)
  vi.advanceTimersByTime(1000)
  expect(giveUp).toHaveBeenCalledTimes(1)
  dispatch([{ type: 'SessionOwnerChanged', session: 's2', owner: CLOUD, epoch: 3, here: false }])
  expect(retry).toHaveBeenCalledTimes(1)
  vi.useRealTimers()
})

test('183 §13 row 2 (Composer): a NotOwner send is taken once — resent after the owner changes, a second refusal or any other error is shown', () => {
  vi.useFakeTimers()
  const notOwner = () => new RpcError(-32013, 'not owner', { NotOwner: { owner: CLOUD } })
  const retried = { current: false }
  const resend = vi.fn()
  const fail = vi.fn()
  expect(retryNotOwner(new Error('boom'), 's1', retried, resend, fail)).toBe(false)
  expect(retried.current).toBe(false)
  expect(retryNotOwner(notOwner(), 's1', retried, resend, fail)).toBe(true)
  expect(resend).not.toHaveBeenCalled() // waits for the change; the composer keeps the draft meanwhile
  dispatch([{ type: 'SessionOwnerChanged', session: 's1', owner: CLOUD, epoch: 2, here: false }])
  expect(resend).toHaveBeenCalledTimes(1)
  // The resend was refused too: no second wait, the caller shows the error.
  expect(retryNotOwner(notOwner(), 's1', retried, resend, fail)).toBe(false)
  vi.advanceTimersByTime(60_000)
  expect(fail).not.toHaveBeenCalled()
  vi.useRealTimers()
})
