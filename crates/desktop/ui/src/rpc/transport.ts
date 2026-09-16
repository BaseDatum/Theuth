/** Socket lifecycle as the store shows it. */
export type ConnState = 'connecting' | 'open' | 'reconnecting' | 'closed'

/**
 * What `RpcClient` speaks over (31 §7.1): text JSON-RPC in, text out. A transport
 * reports `reconnecting` whenever it lost the peer without `close()` being called;
 * the client then re-calls `connect()` with backoff. `connect()` may be called
 * again while a previous attempt is pending; it must return that attempt.
 * `reset()` drops a socket the client has decided is dead (no reply to a probe
 * after the phone slept) and reports `reconnecting` at once, without waiting for
 * WebKit's own close handshake.
 */
export interface Transport {
  connect(): Promise<void>
  send(text: string): void
  onMessage(cb: (text: string) => void): void
  onState(cb: (s: ConnState) => void): void
  reset(): void
  close(): void
}

/** The daemon's own jsonrpsee WebSocket, token as `?token=` (25 §2.1). */
export class LocalTransport implements Transport {
  #url: string
  #ws: WebSocket | null = null
  #pending: Promise<void> | null = null
  #closed = false
  #message: (text: string) => void = () => {}
  #state: (s: ConnState) => void = () => {}

  constructor(url: string, token: string) {
    const u = new URL(url)
    u.searchParams.set('token', token)
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
      this.#pending = null
      this.#state('open')
      resolve()
    }
    ws.onmessage = (m) => this.#message(m.data as string)
    ws.onclose = () => this.#lost(ws, reject)
    this.#pending = promise
    return promise
  }

  #lost(ws: WebSocket, reject?: (e: Error) => void): void {
    if (this.#ws !== ws) return
    this.#ws = null
    this.#pending = null
    reject?.(new Error('connect failed'))
    if (!this.#closed) this.#state('reconnecting')
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

  onState(cb: (s: ConnState) => void): void {
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
