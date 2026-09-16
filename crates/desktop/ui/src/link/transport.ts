// Session-mode device socket (31 §4.3, §7.1): relay `PRESENCE`/`CLOSE`, one IK
// handshake per (re)connect, FIN records in and out. Reconnect timing is the
// `RpcClient`'s; this class only reports `reconnecting` and lets `connect()` be
// called again (a parked socket is reused until the harness comes back).

import type { ConnState, Transport } from '@/rpc/transport'
import { Reassembler, decodeFrame, encodeData, splitRecords } from './frames'
import { type Harness, useHarnesses } from './harnesses'
import { getIdentity, unb64url } from './keys'
import { type Initiator, type Session, handshakeIK, prologue } from './noise'
import { type LinkOptions, SUBPROTOCOL, wsUrl } from './pairing'

const EMPTY = new Uint8Array(0)
const utf8 = new TextEncoder()
const text = new TextDecoder()

/** Message 2 payload of the session handshake. */
export type SessionHello = { name: string; version: string }

export class LinkTransport implements Transport {
  readonly harness: Harness
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
  /** Last socket closed before `open`: the upgrade was refused (relay-origin cookie stale after a relay restart). */
  #refused = false
  #message: (t: string) => void = () => {}
  #state: (s: ConnState) => void = () => {}
  /** Last `SessionHello`, for the switcher. */
  hello: SessionHello | null = null

  constructor(harness: Harness, opts: LinkOptions = {}) {
    this.harness = harness
    this.#opts = opts
  }

  connect(): Promise<void> {
    if (this.#pending) return this.#pending.promise
    if (this.#session) return Promise.resolve()
    this.#closed = false
    this.#pending = Promise.withResolvers<void>()
    if (!this.#ws) void this.#open()
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
    const ws = this.#ws
    this.#ws = null
    ws?.close(1000)
    this.#drop()
    this.#pending?.reject(new Error('closed'))
    this.#pending = null
    this.#state('closed')
  }

  reset(): void {
    const ws = this.#ws
    if (!ws) return
    this.#lost(ws, true)
    ws.close(1000)
  }

  /** The socket `ws` is gone (closed by the peer, or given up on): forget it and report. */
  #lost(ws: WebSocket, opened: boolean): void {
    if (this.#ws !== ws) return
    this.#ws = null
    this.#refused = !opened
    this.#drop()
    const pending = this.#pending
    this.#pending = null
    if (!this.#closed) {
      this.#presence('offline')
      this.#state('reconnecting')
    }
    pending?.reject(new Error('link: socket closed'))
  }

  #presence(p: 'online' | 'offline' | 'connecting'): void {
    useHarnesses.getState().setPresence(this.harness.id, p)
  }

  #drop(): void {
    this.#session = null
    this.#hs = null
    this.#asm.reset()
  }

  async #open(): Promise<void> {
    this.#state('connecting')
    this.#presence('connecting')
    // Served by the relay (31 §7.6) the upgrade needs its `__Host-link` cookie, which a relay
    // restart invalidates (secret random per start → 403 forever). Any page fetch re-issues it.
    if (this.#refused && globalThis.location?.host === this.harness.relayHost) {
      const attempt = this.#pending
      await fetch('/', { cache: 'no-store' }).catch(() => undefined)
      if (this.#pending !== attempt || this.#ws) return // closed (and maybe re-opened) meanwhile
    }
    const WS = this.#opts.ws ?? globalThis.WebSocket
    const ws = new WS(wsUrl(this.harness), [SUBPROTOCOL])
    ws.binaryType = 'arraybuffer'
    this.#ws = ws
    let opened = false
    ws.onopen = () => {
      opened = true
    }
    ws.onmessage = (m) => {
      if (!(m.data instanceof ArrayBuffer)) return
      const bytes = new Uint8Array(m.data)
      // Browsers throw on close codes outside 1000/3000–4999; a throwing catch would poison this chain for good.
      this.#inbound = this.#inbound.then(() => this.#frame(ws, bytes)).catch(() => ws.close())
    }
    ws.onclose = () => this.#lost(ws, opened)
  }

  async #frame(ws: WebSocket, bytes: Uint8Array<ArrayBuffer>): Promise<void> {
    const f = decodeFrame(bytes)
    if (!f || this.#ws !== ws) return
    if (f.op === 'presence') {
      if (f.online) {
        this.#presence('connecting')
        const identity = await (this.#opts.identity ?? getIdentity)()
        this.#drop()
        this.#hs = await handshakeIK({ s: identity, rs: unb64url(this.harness.id) as Uint8Array<ArrayBuffer>, prologue: prologue(this.harness.id) })
        ws.send(encodeData(this.#hs.message))
      } else {
        this.#drop()
        this.#presence('offline')
        this.#state('reconnecting')
      }
    } else if (f.op === 'close') {
      // Any reason (refused = revoked, idle, harness gone): the socket closes; the client retries with backoff.
      ws.close(1000)
    } else if (this.#session) {
      const whole = this.#asm.push(await this.#session.recv.decrypt(EMPTY, f.payload))
      if (whole) this.#message(text.decode(whole))
    } else if (this.#hs) {
      const hs = this.#hs
      this.#hs = null
      this.#session = await hs.finish(f.payload)
      this.hello = JSON.parse(text.decode(this.#session.payload)) as SessionHello
      const { harnesses, add } = useHarnesses.getState()
      const cur = harnesses.find((h) => h.id === this.harness.id) ?? this.harness
      // `name` is the user's local label (Settings › Harnesses); the live name
      // is `hello.name` / `info.name`, shown by the switcher. Never overwrite.
      add({ ...cur, lastSeen: Date.now() })
      this.#presence('online')
      this.#state('open')
      this.#pending?.resolve()
      this.#pending = null
    }
  }
}
