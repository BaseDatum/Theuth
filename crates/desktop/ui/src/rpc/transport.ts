/** Socket lifecycle as the store shows it. */
export type ConnState = 'connecting' | 'open' | 'reconnecting' | 'closed'

/** Why the socket last dropped (176 U5), logged to the engine once it is open again. */
export type DropReason = 'probe-timeout' | 'send-timeout' | 'foreground-reset' | `onclose code=${number} clean=${boolean}` | 'onclose'

/**
 * What `RpcClient` speaks over (31 §7.1): text JSON-RPC in, text out. A transport
 * reports `reconnecting` whenever it lost the peer without `close()` being called
 * (with the close's `why` when it has one); the client then re-calls `connect()`
 * with backoff. `connect()` may be called again while a previous attempt is pending;
 * it must return that attempt. `reset()` drops a socket the client has decided is
 * dead (no reply to a probe after the phone slept) and reports `reconnecting` at
 * once, without waiting for WebKit's own close handshake. `loopback`: its sockets
 * never half-open, so a foreground return does not probe them (176 U2).
 */
export interface Transport {
  readonly loopback?: boolean
  connect(): Promise<void>
  send(text: string): void
  onMessage(cb: (text: string) => void): void
  onState(cb: (s: ConnState, why?: DropReason) => void): void
  reset(): void
  close(): void
}

/** The daemon's own jsonrpsee WebSocket, token as `?token=` (25 §2.1), the window's tag as `?client=` (176 C3). */
export class LocalTransport implements Transport {
  readonly loopback = true
  #url: string
  #ws: WebSocket | null = null
  #pending: Promise<void> | null = null
  #closed = false
  #message: (text: string) => void = () => {}
  #state: (s: ConnState, why?: DropReason) => void = () => {}

  constructor(url: string, token: string, client?: string) {
    const u = new URL(url)
    u.searchParams.set('token', token)
    if (client) u.searchParams.set('client', client)
    this.#url = u.toString()
  }

  connect(): Promise<void> {
    if (this.#ws?.readyState === WebSocket.OPEN) return Promise.resolve()
    if (this.#pending) return this.#pending
    this.#closed = false
    this.#state('connecting')
    const { promise, resolve, reject } = Promise.withResolvers<void>()
    const ws = new WebSocket(this.#url)
    this.#ws = ws
    ws.onopen = () => {
      if (this.#ws !== ws) return
      this.#pending = null
      this.#state('open')
      resolve()
    }
    ws.onmessage = (m) => { if (this.#ws === ws) this.#message(m.data as string) }
    ws.onclose = (e) => this.#lost(ws, reject, `onclose code=${e.code} clean=${e.wasClean}`)
    this.#pending = promise
    return promise
  }

  #lost(ws: WebSocket, reject?: (e: Error) => void, why?: DropReason): void {
    if (this.#ws !== ws) return
    this.#ws = null
    this.#pending = null
    reject?.(new Error('connect failed'))
    if (!this.#closed) this.#state('reconnecting', why)
  }

  reset(): void {
    const ws = this.#ws
    if (!ws) return
    this.#lost(ws)
    ws.close()
  }

  send(text: string): void {
    this.#ws?.send(text)
  }

  onMessage(cb: (text: string) => void): void {
    this.#message = cb
  }

  onState(cb: (s: ConnState, why?: DropReason) => void): void {
    this.#state = cb
  }

  close(): void {
    this.#closed = true
    this.#ws?.close()
    this.#ws = null
    this.#pending = null
    this.#state('closed')
  }
}
