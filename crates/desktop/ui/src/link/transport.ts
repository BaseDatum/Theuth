// Session-mode device socket (31 §4.3, §7.1): relay `PRESENCE`/`CLOSE`, one IK
// handshake per (re)connect, FIN records in and out. Reconnect timing is the
// `RpcClient`'s; this class only reports `reconnecting` and lets `connect()` be
// called again (a parked socket is reused until the engine comes back). Liveness
// (31 §7.8): `CONNECT_MS` to the first PRESENCE, `HANDSHAKE_MS` from PRESENCE 1 to
// ready, and a silent `PARKED_RECYCLE_MS` socket swap while parked.

import { INITIAL, type LinkEvent, type LinkState, reduce } from '@/lib/link-status'
import type { ConnState, Transport } from '@/rpc/transport'
import { CLOSE_REFUSED, Reassembler, decodeFrame, encodeData, splitRecords } from './frames'
import { type Engine, useEngines } from './engines'
import { getIdentity, unb64url } from './keys'
import { type Initiator, type Session, handshakeIK, prologue } from './noise'
import { type LinkOptions, SUBPROTOCOL, wsUrl } from './pairing'

const EMPTY = new Uint8Array(0)
const utf8 = new TextEncoder()
const text = new TextDecoder()

/** From socket creation to the first PRESENCE frame (TCP + TLS + upgrade on cellular; under WebKit's ~60 s). */
export const CONNECT_MS = 15_000
/** From `PRESENCE 1` to `ready` (IK through the relay is tens of ms; the engine itself refuses after 10 s). */
export const HANDSHAKE_MS = 10_000
/** While parked, replace the socket in place: the relay drops a dead one at 45 s (30 s ping + 15 s pong) anyway. */
export const PARKED_RECYCLE_MS = 60_000

/** Message 2 payload of the session handshake. */
export type SessionHello = { name: string; version: string }

export class LinkTransport implements Transport {
  readonly engine: Engine
  #opts: LinkOptions
  #ws: WebSocket | null = null
  #hs: Initiator | null = null
  #session: Session | null = null
  #asm = new Reassembler()
  /** Frames are processed strictly in order: nonces and record reassembly depend on it. */
  #inbound: Promise<void> = Promise.resolve()
  #outbound: Promise<void> = Promise.resolve()
  #pending: PromiseWithResolvers<void> | null = null
  #closed = false
  /** `onopen` fired on the current socket. */
  #opened = false
  /** The one watchdog armed for the current socket (connect, handshake or parked recycle). */
  #watch: ReturnType<typeof setTimeout> | undefined
  /** Last state reported, to dedupe the `reconnecting` a recycled parked socket's PRESENCE 0 would repeat. */
  #last: ConnState = 'closed'
  /** Last socket closed before `open`: the upgrade was refused (relay-origin cookie stale after a relay restart). */
  #refused = false
  /** The engine answered our handshake with `CLOSE refused`: it no longer knows this device's key. Sticky until a handshake succeeds. */
  #rejected = false
  /** What this socket saw, for `lib/link-status` (mirrored into `useEngines.link`). */
  #link: LinkState = INITIAL
  #message: (t: string) => void = () => {}
  #state: (s: ConnState) => void = () => {}
  /** Last `SessionHello`, for the switcher. */
  hello: SessionHello | null = null

  /** The live Noise session's handshake hash (31 §4.2); `null` between handshakes. The §8.3 join proof binds to it. */
  get handshakeHash(): Uint8Array<ArrayBuffer> | null {
    return this.#session?.handshakeHash ?? null
  }

  constructor(engine: Engine, opts: LinkOptions = {}) {
    this.engine = engine
    this.#opts = opts
  }

  connect(): Promise<void> {
    if (this.#pending) return this.#pending.promise
    if (this.#session) return Promise.resolve()
    this.#closed = false
    this.#pending = Promise.withResolvers<void>()
    if (!this.#ws) this.#open().catch((e) => this.#abort(e))
    return this.#pending.promise
  }

  send(t: string): void {
    this.#outbound = this.#outbound.then(async () => {
      const s = this.#session
      if (!s || !this.#ws) return
      for (const rec of splitRecords(utf8.encode(t))) this.#ws.send(encodeData(await s.send.encrypt(EMPTY, rec)))
    })
  }

  onMessage(cb: (t: string) => void): void {
    this.#message = cb
  }

  onState(cb: (s: ConnState) => void): void {
    this.#state = cb
  }

  close(): void {
    this.#closed = true
    this.#disarm()
    const ws = this.#ws
    this.#ws = null
    ws?.close(1000)
    this.#drop()
    this.#pending?.reject(new Error('closed'))
    this.#pending = null
    this.#report('closed')
  }

  reset(): void {
    const ws = this.#ws
    if (!ws) return
    this.#lost(ws, this.#opened)
    ws.close(1000)
  }

  #report(s: ConnState): void {
    this.#last = s
    this.#state(s)
  }

  #disarm(): void {
    clearTimeout(this.#watch)
    this.#watch = undefined
  }

  /** Give up on `ws` after `ms` unless something disarms first (same shape as `reset()`). */
  #arm(ws: WebSocket, ms: number): void {
    this.#disarm()
    this.#watch = setTimeout(() => {
      this.#watch = undefined
      if (this.#ws !== ws) return
      this.#lost(ws, this.#opened)
      ws.close(1000)
    }, ms)
  }

  /** Parked for `PARKED_RECYCLE_MS`: swap the socket silently (`#pending` kept, no state report; 31 §7.8). */
  #recycle(): void {
    this.#watch = undefined
    const ws = this.#ws
    if (!ws || this.#closed) return
    this.#ws = null
    ws.close(1000)
    this.#open(true).catch((e) => this.#abort(e))
  }

  /** `#open()` threw before a socket existed (S4): fail the attempt the way `#lost` would. */
  #abort(e: unknown): void {
    if (this.#closed || this.#ws !== null) return
    this.#disarm()
    this.#drop()
    const pending = this.#pending
    this.#pending = null
    this.#event({ type: 'lost', at: Date.now(), opened: false, reachable: false })
    this.#presence('offline')
    this.#report('reconnecting')
    pending?.reject(e instanceof Error ? e : new Error(String(e)))
  }

  /** The socket `ws` is gone (closed by the peer, or given up on): forget it and report. */
  #lost(ws: WebSocket, opened: boolean): void {
    if (this.#ws !== ws) return
    this.#ws = null
    this.#disarm()
    this.#refused = !opened
    this.#drop()
    const pending = this.#pending
    this.#pending = null
    if (!this.#closed) {
      // The relay served this page (31 §7.6), so it is reachable: a refused upgrade is its rate limit
      // (or a stale cookie, which the refetch in `#open` cures). Cross-origin, the browser hides the
      // HTTP status; every never-opened socket reads as unreachable.
      // ponytail: no probe request from the app shell; add one if 429 on iOS ever needs naming.
      this.#event({ type: 'lost', at: Date.now(), opened, reachable: globalThis.location?.host === this.engine.relayHost })
      this.#presence('offline')
      this.#report('reconnecting')
    }
    pending?.reject(new Error('link: socket closed'))
  }

  #presence(p: 'online' | 'offline' | 'connecting' | 'refused'): void {
    // A refusal outlives the retry loop's `connecting` flickers; only a successful handshake clears it.
    if (this.#rejected && p === 'connecting') return
    useEngines.getState().setPresence(this.engine.id, p)
  }

  #event(e: LinkEvent): void {
    this.#link = reduce(this.#link, e)
    useEngines.getState().setLink(this.engine.id, this.#link)
  }

  #drop(): void {
    this.#session = null
    this.#hs = null
    this.#asm.reset()
  }

  /** `recycle`: a parked socket is being replaced in place — no state/presence/`connecting` report, so counters do not restart. */
  async #open(recycle = false): Promise<void> {
    if (!recycle) {
      this.#report('connecting')
      this.#presence('connecting')
      this.#event({ type: 'connecting', at: Date.now() })
    }
    // Served by the relay (31 §7.6) the upgrade needs its `__Host-link` cookie, which a relay
    // restart invalidates (secret random per start → 403 forever). Any page fetch re-issues it.
    if (this.#refused && globalThis.location?.host === this.engine.relayHost) {
      const attempt = this.#pending
      await fetch('/', { cache: 'no-store' }).catch(() => undefined)
      if (this.#pending !== attempt || this.#ws) return // closed (and maybe re-opened) meanwhile
    }
    const WS = this.#opts.ws ?? globalThis.WebSocket
    const ws = new WS(wsUrl(this.engine), [SUBPROTOCOL])
    ws.binaryType = 'arraybuffer'
    this.#ws = ws
    this.#opened = false
    ws.onopen = () => {
      this.#opened = true
    }
    ws.onmessage = (m) => {
      if (!(m.data instanceof ArrayBuffer)) return
      const bytes = new Uint8Array(m.data)
      // Browsers throw on close codes outside 1000/3000–4999; a throwing catch would poison this chain for good.
      this.#inbound = this.#inbound.then(() => this.#frame(ws, bytes)).catch(() => ws.close())
    }
    ws.onclose = () => this.#lost(ws, this.#opened)
    this.#arm(ws, CONNECT_MS)
  }

  async #frame(ws: WebSocket, bytes: Uint8Array<ArrayBuffer>): Promise<void> {
    const f = decodeFrame(bytes)
    if (!f || this.#ws !== ws) return
    if (f.op === 'presence') {
      this.#event({ type: 'presence', online: f.online, at: Date.now() })
      if (f.online) {
        this.#arm(ws, HANDSHAKE_MS)
        this.#presence('connecting')
        const identity = await (this.#opts.identity ?? getIdentity)(this.engine.kind === 'mesh' ? this.engine.id : undefined)
        if (this.#ws !== ws) return
        this.#drop()
        this.#hs = await handshakeIK({ s: identity, rs: unb64url(this.engine.id) as Uint8Array<ArrayBuffer>, prologue: prologue(this.engine.id) })
        if (this.#ws !== ws) return
        ws.send(encodeData(this.#hs.message))
      } else {
        this.#drop()
        this.#disarm()
        this.#watch = setTimeout(() => this.#recycle(), PARKED_RECYCLE_MS)
        this.#presence('offline')
        if (this.#last !== 'reconnecting') this.#report('reconnecting')
      }
    } else if (f.op === 'close') {
      // `refused` while our handshake is out = the engine does not know this device (revoked, or its
      // devices.yaml is gone). Every other reason — idle, engine gone (4), hub moved (5, 69 decision 6) —
      // is transient; the client retries against the same id and the new hub answers.
      this.#event({ type: 'close', reason: f.reason, at: Date.now(), handshake: this.#hs !== null })
      if (f.reason === CLOSE_REFUSED && this.#hs) {
        this.#rejected = true
        this.#presence('refused')
      }
      this.#opts.onClose?.(f.reason)
      ws.close(1000)
    } else if (this.#session) {
      const plain = await this.#session.recv.decrypt(EMPTY, f.payload)
      if (this.#ws !== ws) return
      const whole = this.#asm.push(plain)
      if (whole) this.#message(text.decode(whole))
    } else if (this.#hs) {
      const hs = this.#hs
      this.#hs = null
      const session = await hs.finish(f.payload)
      if (this.#ws !== ws) return
      this.#disarm()
      this.#session = session
      this.hello = JSON.parse(text.decode(session.payload)) as SessionHello
      const { engines, add } = useEngines.getState()
      const cur = engines.find((h) => h.id === this.engine.id) ?? this.engine
      // `name` is the user's local label (Settings › Mesh); the live name
      // is `hello.name` / `info.name`, shown by the switcher. Never overwrite.
      add({ ...cur, lastSeen: Date.now() })
      this.#rejected = false
      this.#event({ type: 'ready', at: Date.now() })
      this.#presence('online')
      this.#report('open')
      this.#pending?.resolve()
      this.#pending = null
    }
  }
}
