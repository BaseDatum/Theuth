// `LinkTransport` → `useEngines.link` over a scripted fake relay socket: PRESENCE 0 then CLOSE 3 is a stale
// pairing, CLOSE 3 alone is the conn cap, a socket that never opens is unreachable (LINK-MSG); the connect /
// handshake watchdogs and the silent parked recycle (95 LINK-WATCHDOG).
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { failure } from '@/lib/link-status'
import type { ConnState } from '@/rpc/transport'
import { type Engine, useEngines } from './engines'
import { OP_CLOSE, OP_PRESENCE } from './frames'
import { b64url, generateIdentity } from './keys'
import { CONNECT_MS, HANDSHAKE_MS, LinkTransport, PARKED_RECYCLE_MS } from './transport'

class Scripted {
  static last: Scripted
  static reachable = true
  readyState = 0
  binaryType = ''
  onopen: (() => void) | null = null
  onclose: (() => void) | null = null
  onmessage: ((m: { data: ArrayBuffer }) => void) | null = null
  constructor() {
    Scripted.last = this
    queueMicrotask(() => {
      if (!Scripted.reachable) return this.close()
      this.readyState = 1
      this.onopen?.()
    })
  }
  sent = 0
  send() {
    this.sent++
  }
  deliver(...bytes: number[]) {
    this.onmessage?.({ data: Uint8Array.from(bytes).buffer })
  }
  close() {
    if (this.readyState === 3) return
    this.readyState = 3
    queueMicrotask(() => this.onclose?.())
  }
}

const NOW = 5_000_000
/** Drain microtasks (the transport's frame chain) and due timers. */
const flush = () => vi.advanceTimersByTimeAsync(0)
/** The handshake runs real WebCrypto (threadpool, not microtasks): poll on real event-loop turns without moving fake time. */
const sent = (ws: Scripted, n: number) => vi.waitFor(() => expect(ws.sent).toBe(n), { interval: 0 })

beforeEach(() => vi.useFakeTimers({ now: NOW }))
afterEach(() => {
  Scripted.reachable = true
  vi.useRealTimers()
})

async function connect(ws: typeof Scripted = Scripted): Promise<{ h: Engine; t: LinkTransport; states: ConnState[]; p: Promise<void> }> {
  const identity = await generateIdentity()
  const h: Engine = { id: b64url(identity.pub), name: 'Studio Mac', relayHost: 'relay.test', secure: false, pairedAt: 1, lastSeen: 1, order: 0, kind: 'engine' }
  const t = new LinkTransport(h, { ws: ws as unknown as typeof WebSocket, identity: async () => identity })
  const states: ConnState[] = []
  t.onState((s) => states.push(s))
  const p = t.connect()
  p.catch(() => undefined)
  await flush()
  return { h, t, states, p }
}

test('PRESENCE 0 then CLOSE 3 → stale-pairing; the transport drops the socket and the failure sticks', async () => {
  const { h, t } = await connect()
  Scripted.last.deliver(OP_PRESENCE, 0)
  await flush()
  expect(useEngines.getState().link[h.id]).toMatchObject({ phase: 'parked', parkedAt: NOW })
  expect(useEngines.getState().presence[h.id]).toBe('offline')
  Scripted.last.deliver(OP_CLOSE, 3)
  await flush()
  const s = useEngines.getState().link[h.id]!
  expect(s).toMatchObject({ phase: 'lost', sticky: 'stale-pairing', lastClose: { reason: 3, at: NOW }, attempts: 1 })
  expect(failure(s, NOW, h)?.kind).toBe('stale-pairing')
  t.close()
})

test('CLOSE 3 straight after connect → engine-full', async () => {
  const { h, t } = await connect()
  Scripted.last.deliver(OP_CLOSE, 3)
  await flush()
  expect(useEngines.getState().link[h.id]).toMatchObject({ phase: 'lost', sticky: 'engine-full' })
  t.close()
})

test('a socket that never opens → unreachable, with the redial time', async () => {
  Scripted.reachable = false
  const { h, t } = await connect()
  const s = useEngines.getState().link[h.id]!
  expect(s).toMatchObject({ phase: 'lost', sticky: 'unreachable', attempts: 1, retryAt: NOW + 500 })
  expect(failure(s, NOW, h)).toMatchObject({ kind: 'unreachable', detail: 'A proxy may be blocking it. Retrying in 1s.' })
  t.close()
})

test('a socket that opens but never hears PRESENCE is abandoned after CONNECT_MS; a redial opens a new one', async () => {
  const { h, t, states } = await connect()
  const ws = Scripted.last
  await vi.advanceTimersByTimeAsync(CONNECT_MS - 1)
  expect(ws.readyState).toBe(1)
  await vi.advanceTimersByTimeAsync(1)
  expect(ws.readyState).toBe(3)
  expect(states).toEqual(['connecting', 'reconnecting'])
  expect(useEngines.getState().link[h.id]).toMatchObject({ phase: 'lost' })
  expect(useEngines.getState().presence[h.id]).toBe('offline')
  // (e) what `RpcClient.#schedule` does next
  void t.connect().catch(() => undefined)
  await flush()
  expect(Scripted.last).not.toBe(ws)
  expect(Scripted.last.readyState).toBe(1)
  t.close()
})

test('PRESENCE 1 answered by silence is abandoned after HANDSHAKE_MS', async () => {
  const { t, states } = await connect()
  const ws = Scripted.last
  ws.deliver(OP_PRESENCE, 1)
  await sent(ws, 1)
  await vi.advanceTimersByTimeAsync(HANDSHAKE_MS - 1)
  expect(ws.readyState).toBe(1)
  await vi.advanceTimersByTimeAsync(1)
  expect(ws.readyState).toBe(3)
  expect(states).toEqual(['connecting', 'reconnecting'])
  t.close()
})

test('a parked socket is recycled in place every PARKED_RECYCLE_MS without a state report', async () => {
  const { h, t, states, p } = await connect()
  const old = Scripted.last
  old.deliver(OP_PRESENCE, 0)
  await flush()
  const reports = states.length
  const link = useEngines.getState().link[h.id]
  await vi.advanceTimersByTimeAsync(PARKED_RECYCLE_MS)
  const fresh = Scripted.last
  expect(fresh).not.toBe(old)
  expect(old.readyState).toBe(3)
  expect(fresh.readyState).toBe(1)
  expect(useEngines.getState().link[h.id]).toMatchObject({ parkedAt: NOW, attempts: 1 })
  expect(useEngines.getState().link[h.id]).toEqual(link)
  expect(useEngines.getState().presence[h.id]).toBe('offline')
  expect(states.length).toBe(reports)
  fresh.deliver(OP_PRESENCE, 0)
  await flush()
  expect(states.length).toBe(reports)
  fresh.deliver(OP_PRESENCE, 1)
  await sent(fresh, 1)
  expect(t.connect()).toBe(p)
  t.close()
})

test('a WebSocket constructor that throws rejects connect() and reports reconnecting', async () => {
  class Throws extends Scripted {
    static throwing = true
    constructor() {
      if (Throws.throwing) throw new Error('boom')
      super()
    }
  }
  const before = Scripted.last
  const { t, states, p } = await connect(Throws)
  await expect(p).rejects.toThrow('boom')
  expect(states).toEqual(['connecting', 'reconnecting'])
  Throws.throwing = false
  void t.connect().catch(() => undefined)
  await flush()
  expect(Scripted.last).not.toBe(before)
  expect(Scripted.last).toBeInstanceOf(Throws)
  expect(Scripted.last.readyState).toBe(1)
  t.close()
})
