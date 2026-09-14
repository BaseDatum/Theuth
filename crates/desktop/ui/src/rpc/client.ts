import type { WireEvent } from '@/rpc/events'

/** Socket lifecycle as the store shows it. */
export type ConnState = 'connecting' | 'open' | 'reconnecting' | 'closed'

/** A JSON-RPC error object, or a transport failure (`code` -32300). */
export class RpcError extends Error {
  readonly code: number
  readonly data: unknown
  constructor(code: number, message: string, data?: unknown) {
    super(message)
    this.name = 'RpcError'
    this.code = code
    this.data = data
  }
}

const TRANSPORT = -32300
const BACKOFF_MIN_MS = 500
const BACKOFF_MAX_MS = 10_000

type Pending = { resolve: (v: unknown) => void; reject: (e: RpcError) => void }
type Sub = { session: string | null; descendants: boolean | null; onEvent: (e: WireEvent) => void; id: unknown }
type Frame = {
  id?: number
  result?: unknown
  error?: { code: number; message: string; data?: unknown }
  method?: string
  params?: { subscription: unknown; result: WireEvent }
}

/**
 * JSON-RPC 2.0 over one WebSocket (jsonrpsee framing): positional params,
 * numeric ids, subscription items as `subscribe_events` notifications keyed
 * by `params.subscription`. Reconnects with capped exponential backoff and
 * re-issues live subscriptions; the gap between is lost by design (25 §2.1).
 */
export class RpcClient {
  #url: string
  #ws: WebSocket | null = null
  #next = 1
  #pending = new Map<number, Pending>()
  #subs = new Set<Sub>()
  #bySubId = new Map<unknown, Sub>()
  #state: ConnState = 'closed'
  #listeners = new Set<(s: ConnState) => void>()
  #attempt = 0
  #timer: ReturnType<typeof setTimeout> | undefined
  #closed = false

  constructor(url: string, token: string) {
    const u = new URL(url)
    u.searchParams.set('token', token)
    this.#url = u.toString()
  }

  /** Open the socket; resolves once it is open, rejects if the first attempt fails. */
  connect(): Promise<void> {
    this.#closed = false
    this.#setState('connecting')
    return this.#open()
  }

  /** One request; rejects with `RpcError` on an error reply or when the socket is not open. */
  call<T>(method: string, params: unknown[]): Promise<T> {
    const { promise, resolve, reject } = Promise.withResolvers<T>()
    if (this.#ws?.readyState !== WebSocket.OPEN) {
      reject(new RpcError(TRANSPORT, 'not connected'))
      return promise
    }
    const id = this.#next++
    this.#pending.set(id, { resolve: resolve as (v: unknown) => void, reject })
    this.#ws.send(JSON.stringify({ jsonrpc: '2.0', id, method, params }))
    return promise
  }

  /** `subscribe_events`; the returned function unsubscribes. Survives reconnects. */
  async subscribe(
    session: string | null,
    descendants: boolean | null,
    onEvent: (e: WireEvent) => void,
  ): Promise<() => Promise<void>> {
    const sub: Sub = { session, descendants, onEvent, id: null }
    this.#subs.add(sub)
    await this.#issue(sub)
    return async () => {
      this.#subs.delete(sub)
      this.#bySubId.delete(sub.id)
      if (this.#ws?.readyState === WebSocket.OPEN) {
        await this.call('unsubscribe_events', [sub.id]).catch(() => undefined)
      }
    }
  }

  /** Observe state changes; called immediately with the current state. Returns the unlisten. */
  onState(cb: (s: ConnState) => void): () => void {
    this.#listeners.add(cb)
    cb(this.#state)
    return () => this.#listeners.delete(cb)
  }

  /** Close for good: no reconnect, pending calls rejected, subscriptions dropped. */
  close(): void {
    this.#closed = true
    clearTimeout(this.#timer)
    this.#subs.clear()
    this.#bySubId.clear()
    this.#ws?.close()
    this.#ws = null
    this.#failPending('closed')
    this.#setState('closed')
  }

  #open(): Promise<void> {
    const { promise, resolve, reject } = Promise.withResolvers<void>()
    const ws = new WebSocket(this.#url)
    this.#ws = ws
    let opened = false
    ws.onopen = () => {
      opened = true
      this.#attempt = 0
      this.#setState('open')
      for (const sub of this.#subs) void this.#issue(sub)
      resolve()
    }
    ws.onmessage = (m) => this.#receive(JSON.parse(m.data as string) as Frame)
    ws.onclose = () => {
      if (this.#ws !== ws) return
      this.#ws = null
      this.#failPending('connection closed')
      if (!opened) reject(new RpcError(TRANSPORT, 'connect failed'))
      if (this.#closed) return
      this.#setState('reconnecting')
      const delay = Math.min(BACKOFF_MAX_MS, BACKOFF_MIN_MS * 2 ** this.#attempt++)
      this.#timer = setTimeout(() => void this.#open().catch(() => undefined), delay)
    }
    return promise
  }

  async #issue(sub: Sub): Promise<void> {
    const id = await this.call<unknown>('subscribe_events', [sub.session, sub.descendants])
    if (!this.#subs.has(sub)) return
    sub.id = id
    this.#bySubId.set(id, sub)
  }

  #receive(f: Frame): void {
    if (f.method === 'subscribe_events' && f.params) {
      this.#bySubId.get(f.params.subscription)?.onEvent(f.params.result)
      return
    }
    if (f.id === undefined) return
    const p = this.#pending.get(f.id)
    if (!p) return
    this.#pending.delete(f.id)
    if (f.error) p.reject(new RpcError(f.error.code, f.error.message, f.error.data))
    else p.resolve(f.result)
  }

  #failPending(why: string): void {
    for (const p of this.#pending.values()) p.reject(new RpcError(TRANSPORT, why))
    this.#pending.clear()
  }

  #setState(s: ConnState): void {
    if (this.#state === s) return
    this.#state = s
    for (const cb of this.#listeners) cb(s)
  }
}
