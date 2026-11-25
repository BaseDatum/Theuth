import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { createConnection, mirror, routerMounted, watchDrops } from './connection'
import type { ConnState, Retry } from '@/rpc/client'
import type { Info } from "@/rpc/bindings/method.rpc_2dcompose.output"

beforeEach(() => {
  vi.useFakeTimers()
  vi.stubGlobal('window', globalThis)
})
afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

const setup = () => {
  const conn = createConnection()
  conn.setState({ info: {} as Info, state: 'open' })
  const drops = watchDrops(conn)
  const set = (s: 'open' | 'reconnecting' | 'connecting') => {
    conn.getState().setConnState(s)
    return drops.next(s)
  }
  const mounted = () => routerMounted(conn.getState(), true, false)
  return { conn, set, mounted }
}

test('a drop reports its gap once the socket is open again', async () => {
  const { set, mounted } = setup()
  set('reconnecting')
  await vi.advanceTimersByTimeAsync(500)
  set('connecting')
  expect(mounted()).toBe(true)
  await vi.advanceTimersByTimeAsync(2_000)
  expect(set('open')).toBe(2_500)
  expect(set('open')).toBeUndefined()
})

test('the local router stays mounted through a 10 s drop, then gives way until the socket is open again', async () => {
  const { conn, set, mounted } = setup()
  set('reconnecting')
  await vi.advanceTimersByTimeAsync(9_999)
  expect(mounted()).toBe(true)
  await vi.advanceTimersByTimeAsync(1)
  expect(mounted()).toBe(false)
  // A link engine is not bound by the grace: it stays unless refused.
  expect(routerMounted(conn.getState(), false, false)).toBe(true)
  expect(routerMounted(conn.getState(), false, true)).toBe(false)
  set('open')
  expect(mounted()).toBe(true)
  set('reconnecting')
  expect(mounted()).toBe(true)
})

test('no info yet: the startup screen, whatever the state', () => {
  const conn = createConnection()
  conn.setState({ state: 'open' })
  expect(routerMounted(conn.getState(), true, false)).toBe(false)
})

test('open clears the backoff in the store, whatever order the client reports it in', () => {
  const conn = createConnection()
  conn.setState({ state: 'reconnecting', retryAt: Date.now() + 4_000, attempt: 3 })
  conn.getState().setConnState('open')
  expect(conn.getState()).toMatchObject({ state: 'open', retryAt: null, attempt: 0 })
})

/** A client the test drives: `state`/`retry` are what it would answer now, `emit` what it reports. */
const fakeClient = () => {
  const listeners = new Set<(s: ConnState) => void>()
  const c = {
    state: 'closed' as ConnState,
    retry: { at: null, attempt: 0 } as Retry,
    onState(cb: (s: ConnState) => void) {
      listeners.add(cb)
      cb(c.state)
      return () => listeners.delete(cb)
    },
    onRetry: () => () => true,
    emit(s: ConnState) {
      c.state = s
      for (const cb of listeners) cb(s)
    },
  }
  return c
}

test('mirror: a window coming back re-reads the client, so a state it missed does not linger', () => {
  const doc = Object.assign(new EventTarget(), { visibilityState: 'visible' })
  vi.stubGlobal('document', doc)
  vi.stubGlobal('window', Object.assign(new EventTarget(), { setTimeout, clearTimeout }))
  const conn = createConnection()
  const client = fakeClient()
  const ctl = new AbortController()
  const seen: ConnState[] = []
  mirror(conn, client, ctl.signal, (s) => seen.push(s))
  client.emit('reconnecting')
  conn.setState({ retryAt: Date.now() + 2_000, attempt: 1 })
  // Reopened without the store hearing it (the case §4's re-read exists for).
  client.state = 'open'
  doc.dispatchEvent(new Event('visibilitychange'))
  expect(conn.getState()).toMatchObject({ state: 'open', retryAt: null, attempt: 0 })
  expect(seen).toEqual(['closed', 'reconnecting', 'open'])
  // Aborted (a relaunch replaced it): the old client's close no longer reaches the store.
  ctl.abort()
  client.emit('closed')
  doc.dispatchEvent(new Event('visibilitychange'))
  expect(conn.getState().state).toBe('open')
})
