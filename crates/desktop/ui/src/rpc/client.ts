import { SUBSCRIPTIONS, type MethodMap, type TopicEvent } from '@/rpc/bindings/client'
import { receiveStream } from '@/rpc/bindings/receive'
import { capabilitiesSchema, disconnected, rawResult, ready, supports, UnsupportedCapability, validateDocument, verifyHash, type Capabilities, type Discovery, type DiscoveryDocument } from '@/rpc/discovery'
import { readLegacy, readTopic } from '@/rpc/topics'
import { type ConnState, type DropReason, LocalTransport, type Transport } from '@/rpc/transport'

export type { ConnState } from '@/rpc/transport'
/** The pending backoff: `at` the redial time (`null` when none is pending), `attempt` the redials since the last open. */
export type Retry = { at: number | null; attempt: number }

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

type Pending = { method: string; raw: boolean; sub?: Sub; resolve: (v: unknown) => void; reject: (e: Error) => void }
type Sub = {
  method: keyof typeof SUBSCRIPTIONS
  unsubscribe: string
  notification: string
  params: unknown[]
  onItem: (item: unknown) => void
  id: string | number | null
  feed: boolean
  issued: number
  issuing?: Promise<void>
}
const object = (v: unknown): v is Record<string, unknown> => v !== null && typeof v === 'object' && !Array.isArray(v)

/**
 * JSON-RPC 2.0 over one `Transport` (jsonrpsee framing): positional params,
 * numeric ids, negotiated topic or legacy subscription notifications keyed
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
  /** 217 §4: when the pending backoff redials, for the status bar's countdown. */
  #retry: Retry = { at: null, attempt: 0 }
  #retryListeners = new Set<(r: Retry) => void>()
  #probe: ReturnType<typeof setTimeout> | undefined
  #closed = false
  #source: string | undefined
  /** `Date.now()` of the last inbound frame: any reply or notification proves the socket. */
  #heardAt = Number.NEGATIVE_INFINITY
  /** The reason of the `reset()` in progress; the transport's own drop reason otherwise. */
  #resetting: DropReason | undefined
  #drop: { at: number; why: DropReason } | null = null
  #generation = 0
  #discovery = disconnected()
  #discoveryListeners = new Set<(d: Discovery) => void>()
  #documents = new Map<string, DiscoveryDocument>()
  #barrier = Promise.withResolvers<void>()
  #diagnostics = 0

  constructor(transport: Transport, source?: string) {
    this.#t = transport
    this.#source = source
    void this.#barrier.promise.catch(() => undefined)
    transport.onMessage(text => { if (!this.#closed) this.#receive(text) })
    transport.onState((s, why) => {
      if (this.#closed || s === this.#state) return
      const was = this.#state
      if (s === 'reconnecting' && !this.#drop && (was === 'open' || this.#resetting)) this.#drop = { at: Date.now(), why: this.#resetting ?? why ?? 'onclose' }
      const generation = this.#invalidate('connection changed')
      if (this.#closed || generation !== this.#generation) return
      if (s === 'open') this.#setDiscovery({ ...this.#discovery, phase: 'discovering' })
      if (this.#closed || generation !== this.#generation) return
      this.#setState(s)
      if (this.#closed || generation !== this.#generation) return
      if (s === 'open') {
        // A transport that came back on its own (a parked link socket's engine returned) beat the pending redial:
        // cancel it, or the next drop's `#schedule` finds the stale timer and shows no countdown.
        clearTimeout(this.#timer)
        this.#timer = undefined
        this.#attempt = 0
        this.#setRetry(null)
        void this.#negotiate(generation)
      } else if (s === 'reconnecting') this.#schedule()
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

  /** Open/authenticate the transport, then negotiate this generation's discovery readiness. */
  async connect(): Promise<void> {
    this.#closed = false
    await this.#t.connect()
    const generation = this.#generation
    await this.#barrier.promise
    if (generation !== this.#generation) throw new RpcError(TRANSPORT, 'connection changed')
  }

  /** Raw generic calls remain available; ordinary calls wait for discovery and fail missing support locally. */
  async call<T>(method: string, params: unknown[]): Promise<T> {
    if (this.#state !== 'open') throw new RpcError(TRANSPORT, 'not connected')
    const generation = this.#generation
    await this.#barrier.promise
    if (generation !== this.#generation) throw new RpcError(TRANSPORT, 'connection changed')
    if (this.#discovery.phase === 'ready' && !supports(this.#discovery, { methods: [method as keyof MethodMap] })) throw new UnsupportedCapability(method)
    const value = await this.#request<T>(method, params)
    if (generation !== this.#generation) throw new RpcError(TRANSPORT, 'connection changed')
    return value
  }

  /** Versioned topic feed; legacy uses only unambiguous old-ABI tags from the same generated owners. */
  subscribe(session: string | null, descendants: boolean | null, onEvent: (e: TopicEvent) => void): Promise<() => Promise<void>> {
    return this.#track('subscribe_events', [session, descendants], 'unsubscribe_events', onEvent as (item: unknown) => void, true)
  }

  /** Direct streams retain their independent items, buffering and initial snapshots. */
  stream<T>(method: string, params: unknown[], unsubscribe: string, onItem: (item: T) => void): Promise<() => Promise<void>> {
    return this.#track(method, params, unsubscribe, onItem as (item: unknown) => void, false)
  }

  async #track(method: string, params: unknown[], unsubscribe: string, onItem: (item: unknown) => void, feed: boolean): Promise<() => Promise<void>> {
    if (this.#discovery.phase === 'failed') throw new Error(this.#discovery.error)
    if (!Object.hasOwn(SUBSCRIPTIONS, method)) throw new UnsupportedCapability(method)
    const name = method as keyof typeof SUBSCRIPTIONS
    const metadata = SUBSCRIPTIONS[name]
    if (metadata.unsubscribe !== unsubscribe) throw new UnsupportedCapability(`${method} subscription ABI`)
    const sub: Sub = { method: name, params, unsubscribe, notification: metadata.notification, onItem, id: null, issued: -1, feed }
    this.#subs.add(sub)
    try { if (ready(this.#discovery)) await this.#issue(sub) }
    catch (e) { this.#subs.delete(sub); throw e }
    return async () => {
      this.#subs.delete(sub)
      const id = sub.id
      sub.id = null
      this.#bySubId.delete(id)
      if (this.#state === 'open' && id !== null && sub.issued === this.#generation) await this.call(sub.unsubscribe, [id]).catch(() => undefined)
    }
  }

  get generation(): number { return this.#generation }
  /** The current state, for a reader that may have missed an event (a window returning from hidden). */
  get state(): ConnState { return this.#state }
  /** `Date.now()` of the last inbound frame of any kind (`-Infinity` before the first): 217 §4's evidence of liveness. */
  get lastRx(): number { return this.#heardAt }
  get retry(): Retry { return this.#retry }
  get discovery(): Discovery { return this.#discovery }
  onDiscovery(cb: (d: Discovery) => void): () => void {
    this.#discoveryListeners.add(cb)
    cb(this.#discovery)
    return () => this.#discoveryListeners.delete(cb)
  }

  /** Observe state changes; called immediately with the current state. Returns the unlisten. */
  onState(cb: (s: ConnState) => void): () => void {
    this.#listeners.add(cb)
    cb(this.#state)
    return () => this.#listeners.delete(cb)
  }

  /** Observe the backoff deadline; called immediately. Returns the unlisten. */
  onRetry(cb: (r: Retry) => void): () => void {
    this.#retryListeners.add(cb)
    cb(this.#retry)
    return () => this.#retryListeners.delete(cb)
  }

  /** Skip the pending backoff and dial now; a failure schedules the next attempt as usual. */
  retryNow(): void {
    if (this.#closed || !this.#timer) return
    clearTimeout(this.#timer)
    this.#timer = undefined
    this.#setRetry(null)
    this.#t.connect().catch(() => this.#schedule())
  }

  /** Close for good: no reconnect, pending calls rejected, subscriptions dropped. */
  close(): void {
    this.#closed = true
    clearTimeout(this.#timer)
    clearTimeout(this.#probe)
    this.#timer = this.#probe = undefined
    this.#setRetry(null)
    this.#subs.clear()
    this.#bySubId.clear()
    if (typeof document !== 'undefined') {
      document.removeEventListener('visibilitychange', this.#foreground)
      window.removeEventListener('pageshow', this.#foreground)
    }
    this.#t.close()
    this.#invalidate('closed')
    this.#setState('closed')
  }

  #request<T>(method: string, params: unknown[], raw = false, sub?: Sub): Promise<T> {
    const { promise, resolve, reject } = Promise.withResolvers<T>()
    if (this.#state !== 'open') { reject(new RpcError(TRANSPORT, 'not connected')); return promise }
    const id = this.#next++
    this.#pending.set(id, { method, raw, sub, resolve: resolve as (v: unknown) => void, reject })
    try { this.#t.send(JSON.stringify({ jsonrpc: '2.0', id, method, params })) }
    catch (e) { this.#pending.delete(id); reject(new RpcError(TRANSPORT, e instanceof Error ? e.message : String(e))) }
    return promise
  }

  #invalidate(why: string): number {
    const generation = ++this.#generation
    this.#failPending(why)
    this.#bySubId.clear()
    for (const sub of this.#subs) { sub.id = null; sub.issued = -1; sub.issuing = undefined }
    this.#barrier.reject(new RpcError(TRANSPORT, why))
    this.#barrier = Promise.withResolvers<void>()
    void this.#barrier.promise.catch(() => undefined)
    this.#diagnostics = 0
    this.#setDiscovery(disconnected())
    return generation
  }

  #setDiscovery(discovery: Discovery): void {
    this.#discovery = discovery
    for (const cb of this.#discoveryListeners) cb(discovery)
  }

  #failDiscovery(error: unknown, capabilities: Capabilities | null = null): void {
    const message = `Discovery failed: ${error instanceof Error ? error.message : String(error)}. Reconnect or update this app/engine.`
    const barrier = this.#barrier
    this.#setDiscovery({ ...this.#discovery, phase: 'failed', capabilities, document: null, error: message })
    barrier.reject(new Error(message))
  }

  async #negotiate(generation: number): Promise<void> {
    if (generation !== this.#generation || this.#discovery.phase !== 'discovering') return
    let capabilities: Capabilities | null = null
    let discovery: Discovery
    try {
      const value = await this.#request<unknown>('rpc.capabilities', [])
      if (generation !== this.#generation) return
      capabilities = capabilitiesSchema.parse(value)
      if (capabilities.protocolVersion.major !== 1) throw new Error(`Unsupported wire protocol ${capabilities.protocolVersion.major}`)
      let document = this.#documents.get(capabilities.schemaHash)
      if (!document) {
        const raw = await this.#request<string>('rpc.discover', [], true)
        if (generation !== this.#generation) return
        await verifyHash(raw, capabilities.schemaHash)
        if (generation !== this.#generation) return
        document = validateDocument(JSON.parse(raw))
      }
      if (document['x-theuth'].protocolVersion.major !== capabilities.protocolVersion.major || document['x-theuth'].protocolVersion.minor !== capabilities.protocolVersion.minor) throw new Error('Capabilities/document protocol mismatch')
      const declared = document['x-theuth'].plugins, advertised = capabilities.plugins
      if (declared.length !== advertised.length || declared.some((name, i) => name !== advertised[i]!.name)) throw new Error('Capabilities/document plugin mismatch')
      if (generation !== this.#generation || this.#discovery.phase !== 'discovering') return
      if (!this.#documents.has(capabilities.schemaHash)) {
        if (this.#documents.size >= 2) this.#documents.delete(this.#documents.keys().next().value!)
        this.#documents.set(capabilities.schemaHash, document)
      }
      discovery = { phase: 'ready', capabilities, document, unavailable: new Set() }
    } catch (e) {
      if (generation !== this.#generation || this.#discovery.phase !== 'discovering') return
      if (!(e instanceof RpcError && e.code === -32601)) { this.#failDiscovery(e, capabilities); return }
      discovery = { phase: 'legacy', capabilities, document: null, unavailable: new Set() }
    }
    this.#setDiscovery(discovery)
    if (generation !== this.#generation || this.#discovery !== discovery) return
    this.#barrier.resolve()
    for (const sub of this.#subs) void this.#issue(sub).catch(() => undefined)
    this.#reportDrop()
  }

  #malformed = (): void => {
    if (this.#diagnostics++ < 3) console.warn('[rpc] ignored malformed routing item')
  }

  #corrupt(): void {
    this.#malformed()
    if (this.#discovery.phase === 'discovering') {
      const generation = this.#generation
      this.#failDiscovery(new Error('Malformed discovery framing'))
      if (generation === this.#generation) this.#failPending('malformed discovery framing')
    }
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
      this.#setRetry(null)
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
    void this.#request('ping', []).catch(() => undefined)
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
    this.#setRetry(Date.now() + delay)
    this.#timer = setTimeout(() => {
      this.#timer = undefined
      this.#setRetry(null)
      if (this.#closed) return
      this.#t.connect().catch(() => this.#schedule())
    }, delay)
  }

  async #issue(sub: Sub): Promise<void> {
    if (!ready(this.#discovery) || !this.#subs.has(sub)) return
    const generation = this.#generation
    if (sub.issued === generation) return sub.issuing
    sub.issued = generation
    if (sub.feed) {
      sub.method = this.#discovery.phase === 'ready' ? 'subscribe_topics' : 'subscribe_events'
      const metadata = SUBSCRIPTIONS[sub.method]
      sub.unsubscribe = metadata.unsubscribe
      sub.notification = metadata.notification
    }
    if (this.#discovery.phase === 'ready' && !supports(this.#discovery, { methods: [sub.method] })) throw new UnsupportedCapability(sub.method)
    const params = sub.feed && sub.method === 'subscribe_topics' ? [null, ...sub.params] : sub.params
    sub.issuing = this.#request<unknown>(sub.method, params, false, sub).then(async id => {
      if (generation !== this.#generation) return
      if (!this.#subs.has(sub)) await this.call(sub.unsubscribe, [id]).catch(() => undefined)
    })
    return sub.issuing
  }

  #receive(text: string): void {
    this.#heardAt = Date.now()
    let f: unknown
    try { f = JSON.parse(text) } catch { this.#corrupt(); return }
    if (!object(f) || f.jsonrpc !== '2.0') { this.#corrupt(); return }
    if ('method' in f) {
      // Notifications only; an unsolicited server request is never a reply or a client action.
      if ('id' in f) return
      if (typeof f.method !== 'string') { this.#malformed(); return }
      const params = f.params
      if (!object(params) || !Object.hasOwn(params, 'subscription') || (typeof params.subscription !== 'string' && !(typeof params.subscription === 'number' && Number.isFinite(params.subscription)))) { this.#malformed(); return }
      const sub = this.#bySubId.get(params.subscription)
      if (!sub || f.method !== sub.notification) return
      if (!Object.hasOwn(params, 'result')) { this.#malformed(); return }
      if (sub.feed) {
        const event = sub.method === 'subscribe_topics' ? readTopic(params.result, this.#malformed) : readLegacy(params.result, this.#malformed)
        if (event && (this.#discovery.phase === 'legacy' || supports(this.#discovery, { topics: [event.key] }))) sub.onItem(event)
      } else sub.onItem(receiveStream(sub.method, params.result))
      return
    }
    if (typeof f.id !== 'number') return
    const p = this.#pending.get(f.id)
    if (!p) return
    this.#pending.delete(f.id)
    if (f.error !== undefined && Object.hasOwn(f, 'result')) { p.reject(new Error('Ambiguous JSON-RPC result/error')); return }
    if (f.error !== undefined) {
      if (!object(f.error) || !Number.isInteger(f.error.code) || typeof f.error.code !== 'number' || typeof f.error.message !== 'string') { p.reject(new Error('Malformed JSON-RPC error')); return }
      if (f.error.code === -32601 && this.#discovery.phase === 'ready') this.#setDiscovery({ ...this.#discovery, unavailable: new Set([...this.#discovery.unavailable, p.method]) })
      p.reject(new RpcError(f.error.code, f.error.message, f.error.data))
    } else if (!Object.hasOwn(f, 'result')) p.reject(new Error('Missing JSON-RPC result'))
    else if (p.raw) {
      try { p.resolve(rawResult(text)) } catch (e) { p.reject(e instanceof Error ? e : new Error(String(e))) }
    } else {
      if (p.sub) {
        const id = f.result
        if (typeof id !== 'string' && !(typeof id === 'number' && Number.isFinite(id))) { p.reject(new Error('Malformed subscription id')); return }
        // Bind synchronously before resolving: ordered transports may deliver the initial item in the same turn.
        if (this.#subs.has(p.sub)) { p.sub.id = id; this.#bySubId.set(id, p.sub) }
      }
      p.resolve(f.result)
    }
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

  #setRetry(at: number | null): void {
    if (this.#retry.at === at && this.#retry.attempt === this.#attempt) return
    this.#retry = { at, attempt: this.#attempt }
    for (const cb of this.#retryListeners) cb(this.#retry)
  }
}
