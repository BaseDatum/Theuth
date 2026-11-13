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
  const toasts: string[] = []
  const drops = watchDrops(conn, (text) => (toasts.push(text), true))
  const set = (s: 'open' | 'reconnecting' | 'connecting') => {
    conn.getState().setConnState(s)
    return drops.next(s)
  }
  const mounted = () => routerMounted(conn.getState(), true, false)
  return { conn, toasts, set, mounted }
}

test('a drop shorter than 3 s shows no toast at all, and reports its gap', async () => {
  const { toasts, set, mounted } = setup()
  set('reconnecting')
  await vi.advanceTimersByTimeAsync(500)
  set('connecting')
  expect(mounted()).toBe(true)
  await vi.advanceTimersByTimeAsync(2_000)
  expect(set('open')).toBe(2_500)
  await vi.advanceTimersByTimeAsync(20_000)
  expect(toasts).toEqual([])
})

test('past 3 s the lost toast shows, and "Reconnected" follows it', async () => {
  const { toasts, set } = setup()
  set('reconnecting')
  await vi.advanceTimersByTimeAsync(2_999)
  expect(toasts).toEqual([])
  await vi.advanceTimersByTimeAsync(1)
  expect(toasts).toEqual(['Connection lost, reconnecting…'])
  set('open')
  expect(toasts).toEqual(['Connection lost, reconnecting…', 'Reconnected'])
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
