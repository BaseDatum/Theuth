import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { RpcClient } from './client'
import type { ConnState, DropReason, Transport } from './transport'

/** A transport the test drives by hand. */
class Fake implements Transport {
  sent: string[] = []
  resets = 0
  connects = 0
  message: (t: string) => void = () => {}
  state: (s: ConnState, why?: DropReason) => void = () => {}
  constructor(readonly loopback = false) {}
  connect(): Promise<void> {
    this.connects++
    return Promise.resolve()
  }
  send(t: string): void {
    this.sent.push(t)
    const request = JSON.parse(t) as Request
    if (request.method === 'rpc.capabilities') reply(this, { id: request.id, error: { code: -32601, message: 'legacy fixture' } })
  }
  onMessage(cb: (t: string) => void): void {
    this.message = cb
  }
  onState(cb: (s: ConnState, why?: DropReason) => void): void {
    this.state = cb
  }
  reset(): void {
    this.resets++
    this.state('reconnecting')
  }
  close(): void {}
}

// Node has no document; the client listens on it when present.
const doc = Object.assign(new EventTarget(), { visibilityState: 'visible' })
const foreground = () => doc.dispatchEvent(new Event('visibilitychange'))
type Request = { id: number; method: string; params: unknown[] }
const requests = (t: Fake): Request[] => t.sent.map((s) => JSON.parse(s) as Request).filter(r => r.method !== 'rpc.capabilities')
const reply = (t: Fake, frame: object) => t.message(JSON.stringify({ jsonrpc: '2.0', ...frame }))

beforeEach(() => {
  vi.useFakeTimers()
  vi.stubGlobal('document', doc)
  vi.stubGlobal('window', new EventTarget())
})
afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

test('foreground probes an open relay socket with ping and resets it when nothing arrives for 5 s', async () => {
  const t = new Fake()
  new RpcClient(t)
  t.state('open')
  await vi.advanceTimersByTimeAsync(1)
  foreground()
  expect(requests(t).map((r) => r.method)).toEqual(['ping'])
  await vi.advanceTimersByTimeAsync(4_999)
  expect(t.resets).toBe(0)
  await vi.advanceTimersByTimeAsync(1)
  expect(t.resets).toBe(1)
})

test('an error reply to the ping (an engine without it) proves the socket', async () => {
  const t = new Fake()
  new RpcClient(t)
  t.state('open')
  await vi.advanceTimersByTimeAsync(1)
  foreground()
  reply(t, { id: requests(t)[0]?.id, error: { code: -32601, message: 'no such method' } })
  await vi.advanceTimersByTimeAsync(5_000)
  expect(t.resets).toBe(0)
})

test('a notification while the ping waits behind a slow engine proves the socket', async () => {
  const t = new Fake()
  new RpcClient(t)
  t.state('open')
  await vi.advanceTimersByTimeAsync(1)
  foreground()
  await vi.advanceTimersByTimeAsync(4_000)
  reply(t, { method: 'subscribe_events', params: { subscription: 7, result: { type: 'TurnStarted' } } })
  await vi.advanceTimersByTimeAsync(10_000)
  expect(t.resets).toBe(0)
})

test('a loopback transport is not probed on foreground: its sockets do not half-open', async () => {
  const t = new Fake(true)
  new RpcClient(t)
  t.state('open')
  await vi.advanceTimersByTimeAsync(1)
  foreground()
  expect(requests(t)).toEqual([])
  await vi.advanceTimersByTimeAsync(5_000)
  expect(t.resets).toBe(0)
})

test('a loopback transport still skips the backoff on foreground while reconnecting', () => {
  const t = new Fake(true)
  new RpcClient(t)
  t.state('open')
  t.state('reconnecting')
  foreground()
  expect(t.connects).toBe(1)
})

test('the reason of a drop is logged once via client_log when the socket is open again', async () => {
  const t = new Fake(true)
  const c = new RpcClient(t, 'desktop-main')
  t.state('open')
  await vi.advanceTimersByTimeAsync(1)
  c.probe('send-timeout')
  await vi.advanceTimersByTimeAsync(5_000)
  expect(t.resets).toBe(1)
  // The redials (500 ms backoff) fail once with the transport's own close; the first reason stands.
  t.state('connecting')
  t.state('reconnecting', 'onclose code=1006 clean=false')
  await vi.advanceTimersByTimeAsync(1_200)
  t.state('open')
  await vi.advanceTimersByTimeAsync(0)
  const logs = requests(t).filter((r) => r.method === 'client_log')
  expect(logs.map((r) => r.params)).toEqual([['warn', 'desktop-main', 'reconnected after 1200 ms: send-timeout']])
  t.state('reconnecting', 'onclose code=1001 clean=true')
  t.state('open')
  await vi.advanceTimersByTimeAsync(0)
  expect(requests(t).filter((r) => r.method === 'client_log').at(-1)?.params).toEqual([
    'warn',
    'desktop-main',
    'reconnected after 0 ms: onclose code=1001 clean=true',
  ])
  expect(requests(t).filter((r) => r.method === 'client_log')).toHaveLength(2)
})

test('a client without a source logs nothing, and a first dial that fails is not a drop', () => {
  const t = new Fake()
  const quiet = new RpcClient(t)
  t.state('open')
  t.state('reconnecting', 'onclose code=1006 clean=false')
  t.state('open')
  expect(requests(t).filter((r) => r.method === 'client_log')).toEqual([])
  quiet.close()
  const u = new Fake()
  new RpcClient(u, 'island')
  u.state('connecting')
  u.state('reconnecting', 'onclose code=1006 clean=false')
  u.state('open')
  expect(requests(u)).toEqual([])
})

test('foreground while reconnecting skips the backoff', () => {
  const t = new Fake()
  new RpcClient(t)
  t.state('open')
  t.state('reconnecting')
  expect(t.connects).toBe(0)
  foreground()
  expect(t.connects).toBe(1)
  expect(t.resets).toBe(1)
})

test('foreground while connecting resets and redials at once', () => {
  const t = new Fake()
  new RpcClient(t)
  t.state('connecting')
  foreground()
  expect(t.resets).toBe(1)
  expect(t.connects).toBe(1)
})

test('a send in flight when the socket drops is rejected at once, not left hanging', async () => {
  const t = new Fake()
  const c = new RpcClient(t)
  t.state('open')
  await vi.advanceTimersByTimeAsync(1)
  const send = c.call('send_input', ['s1', 'long message', [], null, 'id-1'])
  t.state('reconnecting')
  await expect(send).rejects.toMatchObject({ code: -32300 })
  // While down, a retry fails fast too.
  await expect(c.call('send_input', ['s1', 'long message', [], null, 'id-1'])).rejects.toMatchObject({ message: 'not connected' })
})

test('probe() on a silent socket resets it, which fails the call parked on it', async () => {
  const t = new Fake()
  const c = new RpcClient(t)
  t.state('open')
  await vi.advanceTimersByTimeAsync(1)
  const send = expect(c.call('send_input', ['s1', 'hello', [], null, 'id-1'])).rejects.toMatchObject({ code: -32300 })
  await Promise.resolve()
  c.probe()
  expect(requests(t).map((r) => r.method)).toEqual(['send_input', 'ping'])
  await vi.advanceTimersByTimeAsync(5_000)
  expect(t.resets).toBe(1)
  await send
})
