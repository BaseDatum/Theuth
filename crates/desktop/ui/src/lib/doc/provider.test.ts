// The offline queue of 59 §6.3 against a fake engine (a real Y.Doc) and fake-indexeddb.
import 'fake-indexeddb/auto'
import { IDBFactory } from 'fake-indexeddb'
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest'
import * as Y from 'yjs'
import type { Api } from '@/rpc/bindings/client'
import type { ConnState, RpcClient } from '@/rpc/client'
import { EngineProvider, MAX_UPDATE_B64 } from './provider'
import { DocStore, MAX_BYTES, resetDb } from './queue'

const b64 = (bytes: Uint8Array) => btoa(String.fromCharCode(...bytes))
const unb64 = (s: string) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0))
// fake-indexeddb completes requests on `setImmediate`; draining the macrotask queue (0 ms, no wall-clock
// guess) lets every queued IDB request and the promise chains behind it finish before an assertion.
const settle = async () => {
  for (let i = 0; i < 20; i++) {
    const { promise, resolve } = Promise.withResolvers<void>()
    setTimeout(resolve, 0)
    await promise
  }
}

/** The authority: a Y.Doc, `doc_open(sv)` answers the diff in two chunks, `doc_update` applies. */
class Engine {
  doc = new Y.Doc()
  updates: Uint8Array[] = []
  /** Resolve the pending `doc_update`s by hand when set. */
  hold: Array<() => void> | null = null
  chunk = 8
  lose = false
  /** A pre-184 server: `sv: ''`. */
  oldSv = false
  api = {
    doc_open: async (_id: string, sv: string | null) => {
      const diff = Y.encodeStateAsUpdate(this.doc, sv ? unb64(sv) : undefined)
      const state: string[] = []
      for (let i = 0; i < diff.length; i += this.chunk) state.push(b64(diff.subarray(i, i + this.chunk)))
      return { meta: { title: 't' }, state, awareness: '', sv: this.oldSv ? '' : b64(Y.encodeStateVector(this.doc)), server: 'srv', dual: null }
    },
    doc_update: async (_id: string, update: string) => {
      if (this.hold) {
        const { promise, resolve } = Promise.withResolvers<void>()
        this.hold.push(resolve)
        await promise
      }
      if (this.lose) return // a front door that acks and drops (184 §4.7)
      const u = unb64(update)
      this.updates.push(u)
      Y.applyUpdate(this.doc, u)
    },
    doc_awareness: async () => {},
  } as unknown as Api
  text = () => this.doc.getText('t').toString()
}

/** A client whose connection state the test flips. */
class Client {
  #state: ConnState = 'open'
  #listeners = new Set<(s: ConnState) => void>()
  subscribed = 0
  /** The provider's `doc_subscribe` item handler; a test pushes events through it. */
  item: ((e: unknown) => void) | null = null
  set(s: ConnState) {
    this.#state = s
    for (const cb of this.#listeners) cb(s)
  }
  onState(cb: (s: ConnState) => void) {
    this.#listeners.add(cb)
    cb(this.#state)
    return () => this.#listeners.delete(cb)
  }
  async stream(_m: string, _p: unknown[], _u: string, item: (e: unknown) => void) {
    this.subscribed++
    this.item = item
    return async () => {}
  }
  as = () => this as unknown as RpcClient
}

const type = (p: EngineProvider, s: string) => p.doc.getText('t').insert(p.doc.getText('t').length, s)

beforeEach(() => {
  indexedDB = new IDBFactory()
  resetDb()
})
afterEach(() => vi.restoreAllMocks())

describe('DocStore', () => {
  test('pending FIFO survives a reload in seq order and acks delete rows', async () => {
    const s = new DocStore('h', 'd')
    await s.load()
    const a = s.enqueue(Uint8Array.of(1))
    const b = s.enqueue(Uint8Array.of(2))
    const c = s.enqueue(Uint8Array.of(3))
    await settle()
    await s.ack([a])
    const again = new DocStore('h', 'd')
    const loaded = await again.load()
    expect(loaded.pending.map((p) => [p.seq, p.update[0]])).toEqual([
      [b.seq, 2],
      [c.seq, 3],
    ])
    // A later instance sorts after every persisted row, so a new row never collides with or precedes an old one.
    expect(again.enqueue(Uint8Array.of(4)).seq).toBeGreaterThan(c.seq)
    // Another document on the same engine is not visible here.
    expect((await new DocStore('h', 'other').load()).pending).toEqual([])
  })

  test('state beyond MAX_BYTES (with pending) is dropped with a warning, pending kept', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const s = new DocStore('h', 'd')
    await s.load()
    await s.saveState(Uint8Array.of(9))
    expect((await new DocStore('h', 'd').load()).state).toEqual(Uint8Array.of(9))
    s.enqueue(new Uint8Array(1024))
    await s.saveState(new Uint8Array(MAX_BYTES - 512))
    const loaded = await new DocStore('h', 'd').load()
    expect(loaded.state).toBeNull()
    expect(loaded.pending).toHaveLength(1)
    expect(warn).toHaveBeenCalledOnce()
  })
})

describe('EngineProvider offline queue', () => {
  test('sv round-trip: only the diff comes back, chunked, applied once', async () => {
    const h = new Engine()
    h.doc.getText('t').insert(0, 'shared ')
    const shared = Y.encodeStateAsUpdate(h.doc)
    h.doc.getText('t').insert(7, 'new')
    // What the device already has: the state before `new` (as IndexedDB would hold it).
    const s = new DocStore('h', 'd')
    await s.load()
    await s.saveState(shared)
    const open = vi.spyOn(h.api, 'doc_open')
    const client = new Client()
    const p = new EngineProvider(client.as(), h.api, 'd', 'h')
    const applied: number[] = []
    p.doc.on('update', (u: Uint8Array, origin: unknown) => origin === 'engine' && applied.push(u.length))
    await settle()
    expect(p.status).toEqual({ kind: 'live' })
    expect(p.doc.getText('t').toString()).toBe('shared new')
    const sv = open.mock.calls[0]?.[1]
    expect(Y.decodeStateVector(unb64(sv as string)).size).toBe(1) // not the empty vector: the restored state counted
    const diff = Y.encodeStateAsUpdate(h.doc, unb64(sv as string))
    expect(diff.length).toBeGreaterThan(h.chunk) // the reply really was several chunks
    expect(applied).toEqual([diff.length]) // joined and applied as one update
    p.destroy()
  })

  test('a Meta event renames meta and reaches onEvent (59 §24.9: the guest header follows a rename)', async () => {
    const h = new Engine()
    const client = new Client()
    const p = new EngineProvider(client.as(), h.api, 'd', 'h')
    await settle()
    expect(p.meta?.title).toBe('t')
    const seen: string[] = []
    p.onEvent((e) => seen.push(e.type))
    client.item?.({ type: 'Meta', title: 'Renamed' })
    expect(p.meta?.title).toBe('Renamed')
    expect(seen).toEqual(['Meta'])
    p.destroy()
  })

  test('offline edits queue in IndexedDB, replay in order on reconnect as one update, then ack', async () => {
    const h = new Engine()
    const client = new Client()
    const p = new EngineProvider(client.as(), h.api, 'd', 'h')
    await settle()
    type(p, 'a')
    await settle()
    expect(h.text()).toBe('a')
    expect(h.updates).toHaveLength(1)

    client.set('reconnecting')
    expect(p.status).toEqual({ kind: 'offline', queued: 0 })
    type(p, 'b')
    type(p, 'c')
    type(p, 'd')
    expect(p.status).toEqual({ kind: 'offline', queued: 3 })
    await settle()
    expect((await new DocStore('h', 'd').load()).pending).toHaveLength(3)

    client.set('open')
    await settle()
    expect(p.status).toEqual({ kind: 'live' })
    expect(h.text()).toBe('abcd')
    expect(h.updates).toHaveLength(2) // the three offline updates went as one merged update
    expect((await new DocStore('h', 'd').load()).pending).toEqual([])
    p.destroy()
  })

  test('a killed app replays its persisted FIFO on the next open', async () => {
    const h = new Engine()
    const client = new Client()
    const p = new EngineProvider(client.as(), h.api, 'd', 'h')
    await settle()
    client.set('reconnecting')
    type(p, 'offline')
    await settle()
    p.destroy() // saves the state; the pending row stays
    await settle()
    expect(h.text()).toBe('')

    const p2 = new EngineProvider(new Client().as(), h.api, 'd', 'h')
    await settle()
    expect(p2.status).toEqual({ kind: 'live' })
    expect(p2.doc.getText('t').toString()).toBe('offline')
    expect(h.text()).toBe('offline')
    expect((await new DocStore('h', 'd').load()).pending).toEqual([])
    p2.destroy()
  })

  test('typed while the flush is in flight: sent once, after the flush, never twice', async () => {
    const h = new Engine()
    const client = new Client()
    const p = new EngineProvider(client.as(), h.api, 'd', 'h')
    await settle()
    client.set('reconnecting')
    type(p, 'x')
    await settle()
    h.hold = []
    client.set('open')
    await settle()
    expect(h.hold).toHaveLength(1) // the flush of `x` is in flight
    expect(p.status).toEqual({ kind: 'offline', queued: 0 })
    type(p, 'y') // arrives during the flush
    expect(p.status).toEqual({ kind: 'offline', queued: 1 })
    h.hold[0]?.()
    await settle()
    expect(h.hold).toHaveLength(2) // a second round carried `y`
    h.hold[1]?.()
    h.hold = null
    await settle()
    expect(p.status).toEqual({ kind: 'live' })
    expect(h.text()).toBe('xy')
    expect(h.updates).toHaveLength(2)
    expect((await new DocStore('h', 'd').load()).pending).toEqual([])
    p.destroy()
  })

  test('a failed flush keeps the FIFO for the next attempt', async () => {
    const h = new Engine()
    const client = new Client()
    const p = new EngineProvider(client.as(), h.api, 'd', 'h')
    await settle()
    client.set('reconnecting')
    type(p, 'q')
    await settle()
    const fail = vi.spyOn(h.api, 'doc_update').mockRejectedValueOnce(new Error('connection closed'))
    client.set('open')
    await settle()
    expect(fail).toHaveBeenCalledOnce()
    expect(p.status).toEqual({ kind: 'offline', queued: 1 })
    client.set('reconnecting')
    client.set('open')
    await settle()
    expect(p.status).toEqual({ kind: 'live' })
    expect(h.text()).toBe('q')
    p.destroy()
  })

  test('in-process (local) providers touch no IndexedDB', async () => {
    const h = new Engine()
    const client = new Client()
    const p = new EngineProvider(client.as(), h.api, 'd') // engine omitted = local
    await settle()
    client.set('reconnecting')
    type(p, 'm')
    await settle()
    expect(indexedDB.databases ? await indexedDB.databases() : []).toEqual([])
    client.set('open')
    await settle()
    expect(h.text()).toBe('m')
    p.destroy()
  })

  test('a long offline queue replays in merged batches under 48 KiB, each accepted', async () => {
    const h = new Engine()
    const client = new Client()
    const p = new EngineProvider(client.as(), h.api, 'd', 'h')
    await settle()
    client.set('reconnecting')
    // 40 × 3000 chars ≈ 120 KB of text: far over one update's cap, well under the document's.
    for (let i = 0; i < 40; i++) type(p, 'x'.repeat(3000))
    expect(p.status).toEqual({ kind: 'offline', queued: 40 })
    client.set('open')
    await settle()
    expect(p.status).toEqual({ kind: 'live' })
    expect(h.text().length).toBe(120000)
    expect(h.updates.length).toBeGreaterThan(1) // several batches …
    expect(h.updates.length).toBeLessThan(40) // … but merged, not one per keystroke
    for (const u of h.updates) expect(Math.ceil(u.length / 3) * 4).toBeLessThanOrEqual(MAX_UPDATE_B64)
    expect((await new DocStore('h', 'd').load()).pending).toEqual([])
    p.destroy()
  })

  test('a refused update (UpdateTooLarge / DocTooLarge) closes the provider with the reason instead of retrying', async () => {
    const h = new Engine()
    const cap = 1000
    h.api.doc_update = async (_id: string, update: string) => {
      if (update.length > cap) throw new Error(`UpdateTooLarge: one update may be ${cap} bytes at most, not ${update.length}`)
      const u = unb64(update)
      h.updates.push(u)
      Y.applyUpdate(h.doc, u)
      return null
    }
    const client = new Client()
    const p = new EngineProvider(client.as(), h.api, 'd', 'h')
    await settle()
    type(p, 'ok')
    await settle()
    expect(h.text()).toBe('ok')
    type(p, 'p'.repeat(5000)) // one paste the engine will not take
    await settle()
    expect(p.status).toEqual({ kind: 'closed', reason: 'update too large (48 KiB); reload' })
    expect(h.text()).toBe('ok')
    expect((await new DocStore('h', 'd').load()).pending).toEqual([]) // nothing left to replay on the next open
    // The same on the replay path, with the document cap.
    const h2 = new Engine()
    h2.api.doc_update = async () => {
      throw new Error('DocTooLarge: document would exceed 8388608 bytes')
    }
    const c2 = new Client()
    const p2 = new EngineProvider(c2.as(), h2.api, 'e', 'h')
    await settle()
    c2.set('reconnecting')
    type(p2, 'z')
    c2.set('open')
    await settle()
    expect(p2.status).toEqual({ kind: 'closed', reason: 'document full (8 MiB); reload' })
    expect((await new DocStore('h', 'e').load()).pending).toEqual([])
    p.destroy()
    p2.destroy()
  })
})

describe('EngineProvider state-vector push (184 §4.7, §12.5)', () => {
  const reopen = async (client: Client) => {
    client.set('reconnecting')
    client.set('open')
    await settle()
  }
  const setup = async (h: Engine) => {
    const client = new Client()
    const p = new EngineProvider(client.as(), h.api, 'd', 'h')
    await settle()
    return { client, p }
  }

  test('an update the engine acked but dropped is re-sent on the next open', async () => {
    const h = new Engine()
    const { client, p } = await setup(h)
    h.lose = true
    type(p, 'ab')
    await settle()
    expect(h.text()).toBe('')
    h.lose = false
    await reopen(client)
    expect(h.text()).toBe('ab')
    expect(p.status).toEqual({ kind: 'live' })
    p.destroy()
  })

  test('lost acked updates over 48 KiB are re-sent in several doc_update calls, each under the cap', async () => {
    const h = new Engine()
    const { client, p } = await setup(h)
    h.lose = true
    for (let i = 0; i < 20; i++) type(p, String.fromCharCode(97 + i).repeat(4000))
    await settle()
    h.lose = false
    const update = vi.spyOn(h.api, 'doc_update')
    await reopen(client)
    expect(update.mock.calls.length).toBeGreaterThan(1)
    for (const c of update.mock.calls) expect((c[1] as string).length).toBeLessThanOrEqual(MAX_UPDATE_B64)
    expect(h.text()).toBe(p.doc.getText('t').toString())
    expect(p.status).toEqual({ kind: 'live' })
    p.destroy()
  })

  test('entries the server vector covers are dropped: a later reopen sends nothing', async () => {
    const h = new Engine()
    const { client, p } = await setup(h)
    h.lose = true
    type(p, 'a')
    await settle()
    h.lose = false
    await reopen(client) // pushed; this open's sv did not cover it yet
    await reopen(client) // covered now: dropped from the list
    const update = vi.spyOn(h.api, 'doc_update')
    await reopen(client)
    expect(update).not.toHaveBeenCalled()
    expect(h.text()).toBe('a')
    p.destroy()
  })

  /** An engine whose note was edited with a deletion (every import `Replace` deletes): Yjs then puts a delete set in every update. */
  const withDeletion = () => {
    const h = new Engine()
    h.doc.getText('t').insert(0, 'hello world')
    h.doc.getText('t').delete(5, 6)
    return h
  }

  test('a fresh open of a note with a deletion pushes nothing (184 §16)', async () => {
    const h = withDeletion()
    const update = vi.spyOn(h.api, 'doc_update')
    const { p } = await setup(h)
    expect(p.doc.getText('t').toString()).toBe('hello')
    expect(update).not.toHaveBeenCalled()
    expect(p.status).toEqual({ kind: 'live' })
    p.destroy()
  })

  test('a viewer of a note with a deletion ends readonly and never writes', async () => {
    const h = withDeletion()
    const update = vi.spyOn(h.api, 'doc_update')
    const client = new Client()
    const p = new EngineProvider(client.as(), h.api, 'd', 'h', { name: 'g', color: '#000', role: 'viewer' })
    await settle()
    await reopen(client)
    expect(update).not.toHaveBeenCalled()
    expect(p.status).toEqual({ kind: 'readonly' })
    p.destroy()
  })

  test('a lost delete-only update is still repaired by the catch-all', async () => {
    const h = new Engine()
    const { client, p } = await setup(h)
    type(p, 'abc')
    await settle()
    h.lose = true
    p.doc.getText('t').delete(1, 1)
    await settle()
    expect(h.text()).toBe('abc')
    h.lose = false
    await reopen(client)
    expect(h.text()).toBe('ac')
    p.destroy()
  })

  test('a lost delete-only update is repaired even when the first catch-all push fails', async () => {
    const h = new Engine()
    const { client, p } = await setup(h)
    type(p, 'abc')
    await settle()
    h.lose = true
    p.doc.getText('t').delete(1, 1)
    await settle()
    h.lose = false
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    vi.spyOn(h.api, 'doc_update').mockRejectedValueOnce(new Error('connection lost'))
    await reopen(client)
    expect(p.status.kind).toBe('offline')
    expect(h.text()).toBe('abc')
    await reopen(client)
    expect(h.text()).toBe('ac')
    p.destroy()
  })

  test('keystrokes typed while the catch-all is in flight all arrive', async () => {
    const h = withDeletion()
    const { client, p } = await setup(h)
    type(p, '!')
    await settle()
    // Acked, not yet confirmed by a server vector: the next open owes a catch-all (with the delete set).
    h.hold = []
    client.set('reconnecting')
    client.set('open')
    await settle()
    expect(h.hold.length).toBe(1)
    type(p, 'xyz')
    const held = h.hold
    h.hold = null
    for (const r of held) r()
    await settle()
    expect(h.text()).toBe('hello!xyz')
    expect(p.queued).toBe(0)
    expect(p.status).toEqual({ kind: 'live' })
    p.destroy()
  })

  test.each(['slow', 'hub_moved', 'server_moved', 'NotHub:abc'])('Closed{%s} re-syncs, keeps what is typed meanwhile, and ends live', async (reason) => {
    const h = new Engine()
    const { client, p } = await setup(h)
    const open = vi.spyOn(h.api, 'doc_open')
    const states: string[] = []
    p.onStatus((s) => states.push(s.kind))
    client.item?.({ type: 'Closed', reason })
    // 184 §4.7: the editor stays editable through a re-sync (the caret is not dropped); the keystroke is queued.
    expect(p.status).toEqual({ kind: 'connecting', resync: true })
    type(p, 'x')
    await settle()
    expect(open).toHaveBeenCalledTimes(1)
    expect(client.subscribed).toBe(2)
    expect(states).toEqual(['connecting', 'live'])
    expect(h.text()).toBe('x')
    p.destroy()
  })

  test('Closed{deleted} still closes', async () => {
    const h = new Engine()
    const { client, p } = await setup(h)
    const open = vi.spyOn(h.api, 'doc_open')
    client.item?.({ type: 'Closed', reason: 'deleted' })
    await settle()
    expect(p.status).toEqual({ kind: 'closed', reason: 'deleted' })
    expect(open).not.toHaveBeenCalled()
    p.destroy()
  })

  test('an old server (sv empty): nothing pushed beyond the queue', async () => {
    const h = new Engine()
    h.oldSv = true
    const { client, p } = await setup(h)
    h.lose = true
    type(p, 'ab')
    await settle()
    h.lose = false
    const update = vi.spyOn(h.api, 'doc_update')
    await reopen(client)
    expect(update).not.toHaveBeenCalled()
    expect(h.text()).toBe('')
    expect(p.server).toBe('srv')
    p.destroy()
  })

  test('a Server event updates provider.server / dual and reaches onEvent; open sets them', async () => {
    const h = new Engine()
    const { client, p } = await setup(h)
    expect([p.server, p.dual]).toEqual(['srv', null])
    const seen: string[] = []
    p.onEvent((e) => seen.push(e.type))
    client.item?.({ type: 'Server', server: 'b', dual: 'c' })
    expect([p.server, p.dual]).toEqual(['b', 'c'])
    expect(seen).toEqual(['Server'])
    p.destroy()
  })

  test('single engine, nothing lost: a reopen sends no extra doc_update', async () => {
    const h = new Engine()
    const { client, p } = await setup(h)
    type(p, 'a')
    await settle()
    const update = vi.spyOn(h.api, 'doc_update')
    await reopen(client)
    expect(update).not.toHaveBeenCalled()
    expect(h.updates).toHaveLength(1)
    p.destroy()
  })

  test('a sync refused while the socket stays up (an engine restarted under the page, lease warming up) retries and delivers', async () => {
    const h = new Engine()
    const { client, p } = await setup(h)
    const open = h.api.doc_open.bind(h.api)
    let warming = true
    h.api.doc_open = (async (...args: Parameters<typeof open>) => {
      if (warming) throw Object.assign(new Error('another engine holds the hub lease'), { code: -32011, data: { NotHub: { hub: null } } })
      return open(...args)
    }) as typeof open
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    client.set('reconnecting')
    type(p, 'ab')
    client.set('open')
    await settle()
    expect(p.status).toEqual({ kind: 'offline', queued: 1 })
    warming = false
    await vi.waitFor(() => expect(p.status.kind).toBe('live'), { timeout: 3000 })
    expect(h.text()).toBe('ab')
    p.destroy()
  })
})
