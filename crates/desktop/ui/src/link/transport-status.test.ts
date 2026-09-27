// `LinkTransport` → `useEngines.link` over a scripted fake relay socket: PRESENCE 0 then CLOSE 3 is a stale
// pairing, CLOSE 3 alone is the conn cap, a socket that never opens is unreachable (LINK-MSG).
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { failure } from '@/lib/link-status'
import { type Engine, useEngines } from './engines'
import { OP_CLOSE, OP_PRESENCE } from './frames'
import { b64url, generateIdentity } from './keys'
import { LinkTransport } from './transport'

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
  send() {}
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

beforeEach(() => vi.useFakeTimers({ now: NOW }))
afterEach(() => {
  Scripted.reachable = true
  vi.useRealTimers()
})

async function connect(): Promise<{ h: Engine; t: LinkTransport }> {
  const id = b64url((await generateIdentity()).pub)
  const h: Engine = { id, name: 'Studio Mac', relayHost: 'relay.test', secure: false, pairedAt: 1, lastSeen: 1, order: 0, kind: 'engine' }
  const t = new LinkTransport(h, { ws: Scripted as unknown as typeof WebSocket })
  void t.connect().catch(() => undefined)
  await flush()
  return { h, t }
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
