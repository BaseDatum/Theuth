// Pairing string parsing (31 §4.4) and the pairing handshake (§4.3): IKpsk2
// with the one-time secret, then one transport record so the harness enrols us.

import { container } from '@/lib/platform'
import { CLOSE_REASON, Reassembler, decodeFrame, encodeData, splitRecords } from './frames'
import { type Harness, useHarnesses } from './harnesses'
import { type Identity, b64url, getIdentity, unb64url } from './keys'
import { type Session, handshakeIKpsk2, prologue } from './noise'

/** Everything the string carries plus the derived relay socket location. */
export type PairingKey = {
  relayHost: string
  secure: boolean
  harnessPub: Uint8Array<ArrayBuffer>
  harnessId: string
  secret: Uint8Array<ArrayBuffer>
  name: string
}

/** WebSocket subprotocol both relay sockets negotiate. */
export const SUBPROTOCOL = 'theuth-link-1'

/** Test/script seams: a WebSocket constructor and a device identity (the script has no IndexedDB). */
export type LinkOptions = {
  ws?: typeof WebSocket
  identity?: () => Promise<Identity>
}

/** `wss|ws://<relayHost>/h/<harness id>`. */
export const wsUrl = (h: Pick<Harness, 'relayHost' | 'secure' | 'id'>): string => `${h.secure ? 'wss' : 'ws'}://${h.relayHost}/h/${h.id}`

/**
 * `https://host[:port]/link#1.<b64url(pub ‖ secret)>.<b64url(name)>` or the bare
 * `1.<…>.<…>` (then the relay is the page's own origin). `null` when malformed.
 */
export function parsePairingKey(s: string): PairingKey | null {
  let relayHost: string | undefined
  let secure = true
  let frag = s.trim()
  if (/^https?:\/\//i.test(frag)) {
    let u: URL
    try {
      u = new URL(frag)
    } catch {
      return null
    }
    if (u.pathname !== '/link' || !u.hash) return null
    relayHost = u.host
    secure = u.protocol === 'https:'
    frag = u.hash.slice(1)
  } else if (typeof location !== 'undefined' && location.host) {
    relayHost = location.host
    secure = location.protocol === 'https:'
  }
  if (!relayHost) return null
  const parts = frag.split('.')
  if (parts.length !== 3 || parts[0] !== '1') return null
  const keys = unb64url(parts[1] as string)
  const name = unb64url(parts[2] as string)
  if (!keys || keys.length !== 64 || !name) return null
  const harnessPub = keys.slice(0, 32)
  return { relayHost, secure, harnessPub, harnessId: b64url(harnessPub), secret: keys.slice(32), name: new TextDecoder().decode(name) }
}

/** Binary frames from one relay socket as an awaitable queue. */
export class FrameQueue {
  #queue: Uint8Array<ArrayBuffer>[] = []
  #waiter: ((f: Uint8Array<ArrayBuffer> | null) => void) | null = null
  #closed = false

  constructor(readonly ws: WebSocket) {
    ws.binaryType = 'arraybuffer'
    ws.onmessage = (m) => {
      if (!(m.data instanceof ArrayBuffer)) return
      const bytes = new Uint8Array(m.data)
      if (this.#waiter) {
        const w = this.#waiter
        this.#waiter = null
        w(bytes)
      } else this.#queue.push(bytes)
    }
    ws.onclose = ws.onerror = () => {
      this.#closed = true
      this.#waiter?.(null)
      this.#waiter = null
    }
  }

  /** Next frame, `null` once the socket closed; rejects with `timeoutError` after `ms`. */
  next(ms: number, timeoutError: string): Promise<Uint8Array<ArrayBuffer> | null> {
    const queued = this.#queue.shift()
    if (queued) return Promise.resolve(queued)
    if (this.#closed) return Promise.resolve(null)
    const { promise, resolve, reject } = Promise.withResolvers<Uint8Array<ArrayBuffer> | null>()
    const t = setTimeout(() => {
      this.#waiter = null
      reject(new Error(timeoutError))
    }, ms)
    this.#waiter = (f) => {
      clearTimeout(t)
      resolve(f)
    }
    return promise
  }
}

/** `new WS(url, [SUBPROTOCOL])`, resolved once open; rejects with `relay unreachable`. */
export function openSocket(url: string, WS: typeof WebSocket = globalThis.WebSocket): Promise<WebSocket> {
  const { promise, resolve, reject } = Promise.withResolvers<WebSocket>()
  const ws = new WS(url, [SUBPROTOCOL])
  ws.binaryType = 'arraybuffer'
  ws.onopen = () => resolve(ws)
  ws.onerror = ws.onclose = () => reject(new Error('relay unreachable'))
  return promise
}

const PRESENCE_WAIT_MS = 10_000
const REPLY_WAIT_MS = 15_000
/** The first transport record: any request works; `info` also confirms the RPC path end to end. */
const FIRST_REQUEST = '{"jsonrpc":"2.0","id":1,"method":"info","params":[]}'

/**
 * Pair with the harness in `key`: wait for it to be online, run IKpsk2 with
 * `{ name, platform }`, read `PairAccept`, send one `info` request (the record
 * that enrols us) and await its reply. Adds the harness to the store and returns it.
 * Throws `Error('expired or used key' | 'harness offline' | 'relay unreachable')`.
 */
export async function pair(key: string, deviceName: string, opts: LinkOptions = {}): Promise<Harness> {
  const k = parsePairingKey(key)
  if (!k) throw new Error('expired or used key')
  const identity = await (opts.identity ?? getIdentity)()
  const ws = await openSocket(wsUrl({ relayHost: k.relayHost, secure: k.secure, id: k.harnessId }), opts.ws)
  const frames = new FrameQueue(ws)
  try {
    // Relay sends PRESENCE on connect; parked while the harness is offline.
    for (;;) {
      const f = await frames.next(PRESENCE_WAIT_MS, 'harness offline')
      if (!f) throw new Error('relay unreachable')
      const d = decodeFrame(f)
      if (d?.op === 'presence' && d.online) break
      if (d?.op === 'close') throw new Error(`relay closed: ${CLOSE_REASON[d.reason] ?? d.reason}`)
    }
    const hs = await handshakeIKpsk2(
      {
        s: identity,
        rs: k.harnessPub,
        prologue: prologue(k.harnessId),
        payload: new TextEncoder().encode(JSON.stringify({ name: deviceName, platform: container() })),
      },
      k.secret,
    )
    ws.send(encodeData(hs.message))
    const msg2 = await nextData(frames, 'expired or used key')
    let session: Session
    try {
      session = await hs.finish(msg2)
    } catch {
      throw new Error('expired or used key')
    }
    const accept = JSON.parse(new TextDecoder().decode(session.payload)) as { name?: string }
    // First transport record proves we hold the secret; the harness enrols us on decrypt.
    const [record] = splitRecords(new TextEncoder().encode(FIRST_REQUEST))
    ws.send(encodeData(await session.send.encrypt(new Uint8Array(), record as Uint8Array<ArrayBuffer>)))
    const reply = new Reassembler().push(await session.recv.decrypt(new Uint8Array(), await nextData(frames, 'harness offline')))
    const parsed = JSON.parse(new TextDecoder().decode(reply ?? new Uint8Array())) as { error?: { message: string } }
    if (parsed.error) throw new Error(parsed.error.message)
    const now = Date.now()
    const { harnesses, add } = useHarnesses.getState()
    const h: Harness = {
      id: k.harnessId,
      name: accept.name || k.name,
      relayHost: k.relayHost,
      secure: k.secure,
      pairedAt: now,
      lastSeen: now,
      order: harnesses.reduce((m, x) => Math.max(m, x.order + 1), 0),
    }
    add(h)
    return h
  } finally {
    ws.close(1000)
  }
}

/** The next `DATA` payload; `CLOSE` (the harness refused us) or socket loss → `Error(refusedError)`. */
async function nextData(frames: FrameQueue, refusedError: string): Promise<Uint8Array<ArrayBuffer>> {
  for (;;) {
    const f = await frames.next(REPLY_WAIT_MS, refusedError)
    if (!f) throw new Error(refusedError)
    const d = decodeFrame(f)
    if (d?.op === 'data') return d.payload
    if (d?.op === 'close') throw new Error(refusedError)
    if (d?.op === 'presence' && !d.online) throw new Error('harness offline')
  }
}
