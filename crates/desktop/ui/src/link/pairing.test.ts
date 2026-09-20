// `parsePairingKey` cases, then pairing and a session over an in-process fake
// relay + harness (Noise responder built from the same primitives, so this is a
// protocol-flow check; the byte-level check against published vectors is noise.test.ts).
import { expect, test, vi } from 'vitest'
import { RpcClient } from '@/rpc/client'
import { OP_PRESENCE, RECORD_MAX, Reassembler, decodeFrame, encodeData, splitRecords } from './frames'
import { useHarnesses } from './harnesses'
import { type Identity, b64url, generateIdentity, unb64url } from './keys'
import { type CipherState, SymmetricState, concat, dh, prologue } from './noise'
import { pair, parsePairingKey } from './pairing'
import { LinkTransport } from './transport'

const utf8 = new TextEncoder()
const text = new TextDecoder()
const EMPTY = new Uint8Array(0)

test('parsePairingKey: full URL, scheme → secure, bare form needs an origin, junk is null', () => {
  const pub = new Uint8Array(32).fill(1)
  const secret = new Uint8Array(32).fill(2)
  const frag = `1.${b64url(concat(pub, secret))}.${b64url(utf8.encode('Studio Mac'))}`
  const k = parsePairingKey(`https://relay.example.com:8787/link#${frag}`)
  expect(k).toMatchObject({ relayHost: 'relay.example.com:8787', secure: true, name: 'Studio Mac', harnessId: b64url(pub) })
  expect(k?.harnessPub).toEqual(pub)
  expect(k?.secret).toEqual(secret)
  expect(parsePairingKey(`http://localhost:8787/link#${frag}`)).toMatchObject({ relayHost: 'localhost:8787', secure: false })
  expect(parsePairingKey(` ${frag} `)).toBeNull() // no `location` under node
  expect(parsePairingKey(`https://relay.example.com/other#${frag}`)).toBeNull()
  expect(parsePairingKey(`https://relay.example.com/link#2.${frag.slice(2)}`)).toBeNull()
  expect(parsePairingKey(`https://relay.example.com/link#1.${b64url(pub)}.${b64url(utf8.encode('x'))}`)).toBeNull()
  expect(parsePairingKey('https://relay.example.com/link')).toBeNull()
  expect(parsePairingKey('nonsense')).toBeNull()
})

/** Noise IK / IKpsk2 responder: `msg1 → { payload, rs, reply(payload) → { msg2, send, recv } }`. */
async function respond(s: Identity, harnessId: string, msg1: Uint8Array<ArrayBuffer>, psk?: Uint8Array<ArrayBuffer>) {
  const ss = await SymmetricState.init(psk ? 'Noise_IKpsk2_25519_AESGCM_SHA256' : 'Noise_IK_25519_AESGCM_SHA256')
  await ss.mixHash(prologue(harnessId))
  await ss.mixHash(s.pub)
  const re = msg1.subarray(0, 32)
  await ss.mixHash(re)
  if (psk) await ss.mixKey(re)
  await ss.mixKey(await dh(s.priv, re))
  const rs = await ss.decryptAndHash(msg1.subarray(32, 80))
  await ss.mixKey(await dh(s.priv, rs))
  const payload = await ss.decryptAndHash(msg1.subarray(80))
  return {
    payload,
    rs,
    async reply(p: Uint8Array<ArrayBuffer>) {
      const e = await generateIdentity()
      await ss.mixHash(e.pub)
      if (psk) await ss.mixKey(e.pub)
      await ss.mixKey(await dh(e.priv, re))
      await ss.mixKey(await dh(e.priv, rs))
      if (psk) await ss.mixKeyAndHash(psk)
      const msg2 = concat(e.pub, await ss.encryptAndHash(p))
      const [c1, c2] = await ss.split()
      return { msg2, send: c2, recv: c1 }
    },
  }
}

type Conn = { keys?: { send: CipherState; recv: CipherState }; asm: Reassembler; subs: number; enrol?: string }

/** One harness behind one relay, driving fake device sockets. */
class FakeHarness {
  static: Promise<Identity> = generateIdentity()
  id = ''
  psk: Uint8Array<ArrayBuffer> | null = null
  enrolled: string[] = []
  online = true
  /** `false`: the relay refuses TCP; every new socket closes without opening. */
  reachable = true
  attempts = 0
  sockets = new Set<FakeSocket>()
  requests: string[] = []
  #conns = new Map<FakeSocket, Conn>()

  async ready() {
    this.id = b64url((await this.static).pub)
    return this
  }

  attach(ws: FakeSocket) {
    this.sockets.add(ws)
    this.#conns.set(ws, { asm: new Reassembler(), subs: 0 })
    ws.deliver(Uint8Array.of(OP_PRESENCE, this.online ? 1 : 0))
  }

  setOnline(online: boolean) {
    this.online = online
    for (const ws of this.sockets) {
      this.#conns.set(ws, { asm: new Reassembler(), subs: 0 })
      ws.deliver(Uint8Array.of(OP_PRESENCE, online ? 1 : 0))
    }
  }

  async fromDevice(ws: FakeSocket, bytes: Uint8Array<ArrayBuffer>) {
    const f = decodeFrame(bytes)
    if (f?.op !== 'data' || !this.online) return
    const conn = this.#conns.get(ws)!
    const s = await this.static
    if (!conn.keys) {
      let r = await respond(s, this.id, f.payload).catch(() => null)
      let pairing = false
      if (!r && this.psk) {
        r = await respond(s, this.id, f.payload, this.psk)
        pairing = true
      }
      if (!r || (!pairing && !this.enrolled.includes(b64url(r.rs)))) {
        ws.deliver(Uint8Array.of(0x03, 3))
        ws.close()
        return
      }
      if (pairing) {
        expect(JSON.parse(text.decode(r.payload))).toEqual({ name: 'Test phone', platform: 'web' })
        conn.enrol = b64url(r.rs)
      } else expect(r.payload).toEqual(EMPTY)
      const { msg2, send, recv } = await r.reply(utf8.encode(JSON.stringify(pairing ? { name: 'Fake harness' } : { name: 'Fake harness', version: '0.0' })))
      conn.keys = { send, recv }
      ws.deliver(encodeData(msg2))
      return
    }
    const whole = conn.asm.push(await conn.keys.recv.decrypt(EMPTY, f.payload))
    // Like the real harness: the device row is written (and the secret consumed) only once a transport record decrypts.
    if (conn.enrol) {
      this.enrolled.push(conn.enrol)
      this.psk = null
    }
    conn.enrol = undefined
    if (!whole) return
    const req = JSON.parse(text.decode(whole)) as { id: number; method: string; params: unknown[] }
    this.requests.push(req.method)
    const reply = async (msg: unknown) => {
      for (const rec of splitRecords(utf8.encode(JSON.stringify(msg)))) ws.deliver(encodeData(await conn.keys!.send.encrypt(EMPTY, rec)))
    }
    if (req.method === 'info') await reply({ jsonrpc: '2.0', id: req.id, result: { name: 'Fake harness', version: '0.0' } })
    else if (req.method === 'list_sessions') await reply({ jsonrpc: '2.0', id: req.id, result: [] })
    else if (req.method === 'echo') await reply({ jsonrpc: '2.0', id: req.id, result: req.params[0] })
    else if (req.method === 'subscribe_events') {
      const sub = ++conn.subs
      await reply({ jsonrpc: '2.0', id: req.id, result: sub })
      await reply({ jsonrpc: '2.0', method: 'subscribe_events', params: { subscription: sub, result: { type: 'Lagged', count: 1 } } })
    } else await reply({ jsonrpc: '2.0', id: req.id, error: { code: -32601, message: 'no such method' } })
  }
}

/** The subset of `WebSocket` the link code touches, wired to one `FakeHarness`. */
class FakeSocket {
  static harness: FakeHarness
  readyState = 0
  binaryType = 'blob'
  onopen: (() => void) | null = null
  onmessage: ((m: { data: ArrayBuffer }) => void) | null = null
  onclose: (() => void) | null = null
  onerror: (() => void) | null = null
  url: string

  constructor(url: string, protocols: string[]) {
    this.url = url
    expect(protocols).toEqual(['theuth-link-1'])
    expect(url).toBe(`ws://relay.test/h/${FakeSocket.harness.id}`)
    FakeSocket.harness.attempts++
    queueMicrotask(() => {
      if (!FakeSocket.harness.reachable) {
        this.readyState = 3
        this.onerror?.()
        this.onclose?.()
        return
      }
      this.readyState = 1
      this.onopen?.()
      // PRESENCE arrives as its own task, after the `open` continuation ran.
      setTimeout(() => FakeSocket.harness.attach(this), 0)
    })
  }

  send(bytes: Uint8Array<ArrayBuffer>) {
    void FakeSocket.harness.fromDevice(this, bytes)
  }

  deliver(bytes: Uint8Array<ArrayBuffer>) {
    if (this.readyState !== 1) return
    this.onmessage?.({ data: Uint8Array.from(bytes).buffer })
  }

  close() {
    if (this.readyState === 3) return
    this.readyState = 3
    FakeSocket.harness.sockets.delete(this)
    queueMicrotask(() => this.onclose?.())
  }
}

const WS = FakeSocket as unknown as typeof WebSocket
const key = (h: FakeHarness, secret: Uint8Array<ArrayBuffer>) => `http://relay.test/link#1.${b64url(concat(unb64url(h.id) as Uint8Array<ArrayBuffer>, secret))}.${b64url(utf8.encode('Fake'))}`
const until = (cond: () => boolean, timeout = 1000) => vi.waitFor(() => expect(cond()).toBe(true), { timeout })

test('pair: IKpsk2, PairAccept name, first record enrols the device; wrong secret is rejected', async () => {
  const h = await new FakeHarness().ready()
  FakeSocket.harness = h
  const device = await generateIdentity()
  const identity = () => Promise.resolve(device)
  h.psk = crypto.getRandomValues(new Uint8Array(32))
  await expect(pair(key(h, new Uint8Array(32).fill(9)), 'Test phone', { ws: WS, identity })).rejects.toThrow('expired or used key')
  expect(h.enrolled).toEqual([])
  const harness = await pair(key(h, h.psk), 'Test phone', { ws: WS, identity })
  expect(harness).toMatchObject({ id: h.id, name: 'Fake harness', relayHost: 'relay.test', secure: false })
  expect(h.enrolled).toEqual([b64url(device.pub)])
  expect(h.requests).toEqual(['info'])
  expect(useHarnesses.getState().harnesses.map((x) => x.id)).toContain(h.id)
  expect(h.sockets.size).toBe(0)
  // the key is single use: no pending secret → the harness refuses
  await expect(pair(key(h, new Uint8Array(32)), 'Test phone', { ws: WS, identity })).rejects.toThrow('expired or used key')
})

test('session: IK over the relay, RPC round trips, large records split/reassemble, presence flips reconnect', async () => {
  const h = await new FakeHarness().ready()
  FakeSocket.harness = h
  const device = await generateIdentity()
  h.enrolled.push(b64url(device.pub))
  const harness = { id: h.id, name: 'old name', relayHost: 'relay.test', secure: false, pairedAt: 0, lastSeen: 0, order: 0 }
  useHarnesses.getState().add(harness)
  const t = new LinkTransport(harness, { ws: WS, identity: () => Promise.resolve(device) })
  const client = new RpcClient(t)
  const states: string[] = []
  client.onState((s) => states.push(s))
  const events: unknown[] = []
  void client.subscribe(null, null, (e) => events.push(e))
  await client.connect()
  expect(states).toEqual(['closed', 'connecting', 'open'])
  expect(useHarnesses.getState().presence[h.id]).toBe('online')
  // the local label is the user's; the live name arrives in the hello
  expect(useHarnesses.getState().harnesses.find((x) => x.id === h.id)?.name).toBe('old name')
  expect(t.hello?.name).toBe('Fake harness')
  expect(await client.call('list_sessions', [])).toEqual([])
  await until(() => events.length === 1)
  const big = 'x'.repeat(RECORD_MAX * 2 + 7)
  expect(await client.call('echo', [big])).toBe(big)

  h.setOnline(false)
  await until(() => states.at(-1) === 'reconnecting')
  expect(useHarnesses.getState().presence[h.id]).toBe('offline')
  await expect(client.call('list_sessions', [])).rejects.toThrow('not connected')
  h.setOnline(true)
  await until(() => states.at(-1) === 'open')
  await until(() => events.length === 2) // re-subscribed after the new handshake
  expect(h.requests.filter((m) => m === 'subscribe_events')).toHaveLength(2)
  client.close()
  expect(states.at(-1)).toBe('closed')
  expect(h.sockets.size).toBe(0)
})

test('session: raw socket close (relay restart) → reconnecting → new socket → open', async () => {
  const h = await new FakeHarness().ready()
  FakeSocket.harness = h
  const device = await generateIdentity()
  h.enrolled.push(b64url(device.pub))
  const harness = { id: h.id, name: 'x', relayHost: 'relay.test', secure: false, pairedAt: 0, lastSeen: 0, order: 0 }
  useHarnesses.getState().add(harness)
  const client = new RpcClient(new LinkTransport(harness, { ws: WS, identity: () => Promise.resolve(device) }))
  const states: string[] = []
  client.onState((s) => states.push(s))
  await client.connect()
  const first = [...h.sockets][0]!
  // TCP close, no CLOSE frame: the relay process died.
  first.readyState = 3
  h.sockets.delete(first)
  first.onclose?.()
  await until(() => states.at(-1) === 'reconnecting')
  expect(h.sockets.size).toBe(0)
  await until(() => states.at(-1) === 'open')
  expect(h.sockets.size).toBe(1)
  expect([...h.sockets][0]).not.toBe(first)
  expect(await client.call('list_sessions', [])).toEqual([])
  client.close()
})

test('session: relay refuses two upgrades, then accepts → open; served from the relay origin the cookie is refetched first', async () => {
  const h = await new FakeHarness().ready()
  FakeSocket.harness = h
  const device = await generateIdentity()
  h.enrolled.push(b64url(device.pub))
  const harness = { id: h.id, name: 'x', relayHost: 'relay.test', secure: false, pairedAt: 0, lastSeen: 0, order: 0 }
  useHarnesses.getState().add(harness)
  // The web build: page origin = relay; a relay restart rotates the `__Host-link` secret → every upgrade is 403 until a page fetch.
  vi.stubGlobal('location', { host: 'relay.test' })
  const fetchMock = vi.fn(() => Promise.resolve(new Response('')))
  vi.stubGlobal('fetch', fetchMock)
  try {
    h.reachable = false
    const client = new RpcClient(new LinkTransport(harness, { ws: WS, identity: () => Promise.resolve(device) }))
    const states: string[] = []
    client.onState((s) => states.push(s))
    await expect(client.connect()).rejects.toThrow('socket closed')
    expect(fetchMock).not.toHaveBeenCalled() // first attempt: nothing was refused yet
    await until(() => h.attempts >= 2, 5_000)
    expect(fetchMock).toHaveBeenCalledWith('/', { cache: 'no-store' })
    h.reachable = true
    await until(() => states.at(-1) === 'open', 5_000)
    expect(await client.call('list_sessions', [])).toEqual([])
    client.close()
  } finally {
    vi.unstubAllGlobals()
  }
})
