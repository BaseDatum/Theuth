import type { WireEvent } from '@/rpc/events'
import { type ConnState, type DropReason, LocalTransport, type Transport } from '@/rpc/transport'

export type { ConnState } from '@/rpc/transport'

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
/** A call the socket never carried (`not connected`, `connection closed`): worth retrying once the client is `open` again. */
export const isTransportError = (e: unknown): e is RpcError => e instanceof RpcError && e.code === TRANSPORT
/** Redial delay after `attempt` failed sockets in a row: 500 ms doubling to 10 s. `link-status` shows it. */
export const backoff = (attempt: number): number => Math.min(10_000, 500 * 2 ** attempt)
/** A probe that saw no inbound frame for this long means the socket died while the phone slept. */
const PROBE_MS = 5_000

type Pending = { resolve: (v: unknown) => void; reject: (e: RpcError) => void }
type Sub = {
  method: string
  unsubscribe: string
  params: unknown[]
  onItem: (item: never) => void
  id: unknown
}
type Frame = {
  id?: number
  result?: unknown
  error?: { code: number; message: string; data?: unknown }
  method?: string
  params?: { subscription: unknown; result: unknown }
}

/**
 * JSON-RPC 2.0 over one `Transport` (jsonrpsee framing): positional params,
 * numeric ids, subscription items as `subscribe_events` notifications keyed
 * by `params.subscription`. When the transport reports `reconnecting` it is
 * reconnected with capped exponential backoff and live subscriptions are
 * re-issued; the gap between is lost by design (25 §2.1).
 *
 * Coming back to the foreground (31 §7.3) skips the backoff, and probes an
 * `open` relay socket with `ping`: iOS suspends the process and its sockets without
 * telling the page, so a dead one otherwise looks open and every call hangs. A
 * loopback transport is not probed there: its sockets do not half-open (176 U2).
 * With a `source` (the 176 C3 tag), every reconnect is reported once via `client_log`.
 */
export class RpcClient {
  #t: Transport
  #next = 1
  #pending = new Map<number, Pending>()
  #subs = new Set<Sub>()
  #bySubId = new Map<unknown, Sub>()
  #state: ConnState = 'closed'
  #listeners = new Set<(s: ConnState) => void>()
  #attempt = 0
  #timer: ReturnType<typeof setTimeout> | undefined
  #probe: ReturnType<typeof setTimeout> | undefined
  #closed = false
  #source: string | undefined
  /** `Date.now()` of the last inbound frame: any reply or notification proves the socket. */
  #heardAt = Number.NEGATIVE_INFINITY
  /** The reason of the `reset()` in progress; the transport's own drop reason otherwise. */
  #resetting: DropReason | undefined
  #drop: { at: number; why: DropReason } | null = null

  constructor(transport: Transport, source?: string) {
    this.#t = transport
    this.#source = source
    transport.onMessage((text) => this.#receive(JSON.parse(text) as Frame))
    transport.onState((s, why) => {
      const was = this.#state
      this.#setState(s)
      if (s === 'open') {
        this.#attempt = 0
        for (const sub of this.#subs) void this.#issue(sub).catch(() => undefined)
        this.#reportDrop()
      } else if (s === 'reconnecting') {
        if (!this.#drop && (was === 'open' || this.#resetting)) this.#drop = { at: Date.now(), why: this.#resetting ?? why ?? 'onclose' }
        this.#failPending('connection closed')
        this.#schedule()
      }
    })
    // Tests run without a document.
    if (typeof document !== 'undefined') {
      document.addEventListener('visibilitychange', this.#foreground)
      window.addEventListener('pageshow', this.#foreground)
    }
  }

  /** The daemon's own socket (25 §2.1); `source` tags it (176 C3) and names it in `client_log`. */
  static local(url: string, token: string, source?: string): RpcClient {
    return new RpcClient(new LocalTransport(url, token, source), source)
  }

  /** Open the transport; resolves once open, rejects if the first attempt fails (retries continue). */
  connect(): Promise<void> {
    this.#closed = false
    return this.#t.connect()
  }

  /** One request; rejects with `RpcError` on an error reply or when not connected. */
  call<T>(method: string, params: unknown[]): Promise<T> {
    const { promise, resolve, reject } = Promise.withResolvers<T>()
    if (this.#state !== 'open') {
      reject(new RpcError(TRANSPORT, 'not connected'))
      return promise
    }
    const id = this.#next++
    this.#pending.set(id, { resolve: resolve as (v: unknown) => void, reject })
    this.#t.send(JSON.stringify({ jsonrpc: '2.0', id, method, params }))
    return promise
  }

  /**
   * `subscribe_events`; the returned function unsubscribes. Survives reconnects,
   * and may be called before the transport is open (issued on `open`).
   */
  subscribe(
    session: string | null,
    descendants: boolean | null,
    onEvent: (e: WireEvent) => void,
  ): Promise<() => Promise<void>> {
    return this.stream('subscribe_events', [session, descendants], 'unsubscribe_events', onEvent)
  }

  /**
   * Any jsonrpsee subscription pair: items arrive at `onItem`; the returned function unsubscribes.
   * Re-issued on every reconnect (the server starts the stream over), so `onItem` sees its first item again.
   */
  async stream<T>(
    method: string,
    params: unknown[],
    unsubscribe: string,
    onItem: (item: T) => void,
  ): Promise<() => Promise<void>> {
    const sub: Sub = { method, params, unsubscribe, onItem: onItem as (item: never) => void, id: null }
    this.#subs.add(sub)
    if (this.#state === 'open') await this.#issue(sub)
    return async () => {
      this.#subs.delete(sub)
      this.#bySubId.delete(sub.id)
      if (this.#state === 'open' && sub.id !== null) {
        await this.call(sub.unsubscribe, [sub.id]).catch(() => undefined)
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
    clearTimeout(this.#probe)
    this.#timer = this.#probe = undefined
    this.#subs.clear()
    this.#bySubId.clear()
    if (typeof document !== 'undefined') {
      document.removeEventListener('visibilitychange', this.#foreground)
      window.removeEventListener('pageshow', this.#foreground)
    }
    this.#t.close()
    this.#failPending('closed')
    this.#setState('closed')
  }

  #foreground = (): void => {
    if (this.#closed || document.visibilityState !== 'visible') return
    if (this.#state === 'reconnecting' || this.#state === 'connecting') {
      // A socket parked on the relay (engine offline), mid-connect or mid-handshake when iOS suspended may have
      // died in the sleep; replace it at once instead of after the transport's watchdog. A fresh one costs one upgrade.
      this.#reset('foreground-reset')
      clearTimeout(this.#timer)
      this.#timer = undefined
      this.#attempt = 0
      this.#t.connect().catch(() => this.#schedule())
    } else if (!this.#t.loopback) this.probe()
  }

  /**
   * Prove an `open` socket with `ping`, or reset it when no frame at all arrived for `PROBE_MS`: a dead socket the OS
   * never closed (iOS suspend, a Wi-Fi drop) otherwise looks open and every call hangs. Any reply (even an error from
   * an engine without `ping`) or notification proves it, so a slow handler never drops a healthy socket. Also what a
   * timed-out send calls (70 SEND-NEVER-LOSES), with `send-timeout`.
   */
  probe(why: DropReason = 'probe-timeout'): void {
    if (this.#state !== 'open' || this.#probe) return
    const started = Date.now()
    this.#probe = setTimeout(() => {
      this.#probe = undefined
      if (this.#heardAt < started) this.#reset(why)
    }, PROBE_MS)
    void this.call('ping', []).catch(() => undefined)
  }

  #reset(why: DropReason): void {
    this.#resetting = why
    this.#t.reset()
    this.#resetting = undefined
  }

  #reportDrop(): void {
    const drop = this.#drop
    this.#drop = null
    if (!drop || !this.#source) return
    void this.call('client_log', ['warn', this.#source, `reconnected after ${Date.now() - drop.at} ms: ${drop.why}`]).catch(() => undefined)
  }

  #schedule(): void {
    if (this.#closed || this.#timer) return
    const delay = backoff(this.#attempt++)
    this.#timer = setTimeout(() => {
      this.#timer = undefined
      if (this.#closed) return
      this.#t.connect().catch(() => this.#schedule())
    }, delay)
  }

  async #issue(sub: Sub): Promise<void> {
    const id = await this.call<unknown>(sub.method, sub.params)
    if (!this.#subs.has(sub)) return
    sub.id = id
    this.#bySubId.set(id, sub)
  }

  #receive(f: Frame): void {
    this.#heardAt = Date.now()
    if (f.params && 'subscription' in f.params) {
      this.#bySubId.get(f.params.subscription)?.onItem(f.params.result as never)
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
