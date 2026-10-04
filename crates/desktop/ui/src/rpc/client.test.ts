import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { RpcClient } from './client'
import type { ConnState, Transport } from './transport'

/** A transport the test drives by hand. */
class Fake implements Transport {
  sent: string[] = []
  resets = 0
  connects = 0
  message: (t: string) => void = () => {}
  state: (s: ConnState) => void = () => {}
  connect(): Promise<void> {
    this.connects++
    return Promise.resolve()
  }
  send(t: string): void {
    this.sent.push(t)
  }
  onMessage(cb: (t: string) => void): void {
    this.message = cb
  }
  onState(cb: (s: ConnState) => void): void {
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
type Request = { id: number; method: string }
const requests = (t: Fake): Request[] => t.sent.map((s) => JSON.parse(s) as Request)

beforeEach(() => {
  vi.useFakeTimers()
  vi.stubGlobal('document', doc)
  vi.stubGlobal('window', new EventTarget())
})
afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

test('foreground probes an open socket and resets it when nothing answers', async () => {
  const t = new Fake()
  new RpcClient(t)
  t.state('open')
  foreground()
  expect(requests(t).map((r) => r.method)).toEqual(['info'])
  await vi.advanceTimersByTimeAsync(5_000)
  expect(t.resets).toBe(1)
})

test('any reply to the probe keeps the socket', async () => {
  const t = new Fake()
  new RpcClient(t)
  t.state('open')
  foreground()
  const id = requests(t)[0]?.id
  t.message(JSON.stringify({ jsonrpc: '2.0', id, error: { code: -32601, message: 'no such method' } }))
  await vi.advanceTimersByTimeAsync(5_000)
  expect(t.resets).toBe(0)
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
  const send = c.call('send_input', ['s1', 'long message', [], null, 'id-1'])
  t.state('reconnecting')
  await expect(send).rejects.toMatchObject({ code: -32300, message: 'connection closed' })
  // While down, a retry fails fast too.
  await expect(c.call('send_input', ['s1', 'long message', [], null, 'id-1'])).rejects.toMatchObject({ message: 'not connected' })
})

test('probe() on a silent socket resets it, which fails the call parked on it', async () => {
  const t = new Fake()
  const c = new RpcClient(t)
  t.state('open')
  const send = expect(c.call('send_input', ['s1', 'hello', [], null, 'id-1'])).rejects.toMatchObject({ code: -32300 })
  c.probe()
  expect(requests(t).map((r) => r.method)).toEqual(['send_input', 'info'])
  await vi.advanceTimersByTimeAsync(5_000)
  expect(t.resets).toBe(1)
  await send
})
