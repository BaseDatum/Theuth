import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { createConnection, routerMounted, watchDrops } from './connection'
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
