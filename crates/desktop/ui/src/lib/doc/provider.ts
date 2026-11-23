import { Awareness, applyAwarenessUpdate, encodeAwarenessUpdate, removeAwarenessStates } from 'y-protocols/awareness'
import * as Y from 'yjs'
import { LOCAL } from '@/link/engines'
import type { Api } from '@/rpc/bindings/client'
import type { DocEvent } from "@/rpc/bindings/method.document.output"
import type { DocId } from "@/rpc/bindings/method.agent.output"
import type { DocMeta } from "@/rpc/bindings/method.document.output"
import type { Role } from "@/rpc/bindings/method.agent.output"
import type { Who } from "@/rpc/bindings/method.document.output"
import { RpcError, type RpcClient } from '@/rpc/client'
import { ready, supports, UnsupportedCapability } from '@/rpc/discovery'
import { DocStore, type Pending } from './queue'
import { decode, encode } from '../base64'

/** Origin tag of every change the engine sent us; never echoed back. */
const ENGINE = 'engine'
/** Origin of the IndexedDB state applied at start: already ours, already (or pending to be) the engine's. */
const RESTORE = 'restore'
/** The whole Y.Doc is written to IndexedDB this long after the last change (link devices only). */
const SAVE_MS = 1000
/** A failed `#sync` with the socket still up is retried this long after (no reconnect will retry it). */
const SYNC_RETRY_MS = 1000
/** After this many failed retries in a row, the retry waits 5-10 s instead (jittered). */
const SYNC_RETRY_FAST = 10
/** One `doc_update` over the link, base64 (59 §4.7; `document::MAX_UPDATE_B64`). The replay merges the queue in batches under it. */
export const MAX_UPDATE_B64 = 48 * 1024
/** The engine refused an update for good (59 §11 limits): retrying cannot help, the local copy has forked. */
const REFUSED = /^(UpdateTooLarge|DocTooLarge)\b/
/** Bytes of acked updates kept for the state-vector resend. */
const MAX_UNCONFIRMED = 1024 * 1024

export type DocStatus =
  /** `resync`: an editor's re-sync after its server moved (184 §12.5): it keeps taking keystrokes, queued until live. */
  | { kind: 'connecting'; resync?: boolean }
  | { kind: 'live' }
  | { kind: 'offline'; queued: number }
  | { kind: 'readonly' }
  | { kind: 'closed'; reason: string }

/** Who wrote a set of blocks, from a `DocEvent::Authored` (59 §5.5). */
export type Authored = { blocks: string[]; by: Who; at: number }

/** The agent's awareness state while it edits (59 §6.4, §8.5): `{ agent: { doc, blocks, at } }` plus `name`/`color`. */
export type AgentPresence = { name: string; color: string; blocks: string[]; at: number }

/** A share-link member's identity as `doc_join` answered it (59 §8.3, §8.5). */
export type GuestUser = { name: string; color: string; role: Role }

type AgentState = { name?: unknown; color?: unknown; agent: { doc: string; blocks: string[]; at: number } }

/**
 * Awareness whose `user` field is pinned (59 §8.5): BlockNote writes `user` from the host's
 * `deviceName()` when the editor is created; a guest's name and colour are the engine's
 * (`Joined`), so every write of `user` lands as those. `role` rides beside it.
 */
class PinnedUserAwareness extends Awareness {
  readonly #user: { name: string; color: string }

  constructor(doc: Y.Doc, guest: GuestUser) {
    super(doc)
    this.#user = { name: guest.name, color: guest.color }
    super.setLocalStateField('user', this.#user)
    super.setLocalStateField('role', guest.role)
  }

  override setLocalStateField(field: string, value: unknown): void {
    super.setLocalStateField(field, field === 'user' ? this.#user : value)
  }
}

function isAgentState(state: unknown): state is AgentState {
  if (!state || typeof state !== 'object' || !('agent' in state)) return false
  const agent = state.agent
  return (
    !!agent &&
    typeof agent === 'object' &&
    'doc' in agent &&
    typeof agent.doc === 'string' &&
    'blocks' in agent &&
    Array.isArray(agent.blocks) &&
    'at' in agent &&
    typeof agent.at === 'number'
  )
}

const b64 = encode
const unb64 = decode

/** The structs (client, clock, length) an update carries; `Y.encodeStateVectorFromUpdate` is empty for one that does not start at clock 0. */
const structs = (update: Uint8Array) => Y.decodeUpdate(update).structs as Array<{ id: { client: number; clock: number }; length: number }>

/** A v1 state vector from a clock map (`Y.encodeStateVector` only reads a doc): varuint count, then client/clock varuints. */
function svBytes(clocks: Map<number, number>): Uint8Array {
  const out: number[] = []
  const put = (n: number) => {
    for (; n > 127; n = Math.floor(n / 128)) out.push(128 | n % 128)
    out.push(n)
  }
  put(clocks.size)
  for (const [client, clock] of clocks) {
    put(client)
    put(clock)
  }
  return Uint8Array.from(out)
}

/**
 * A Yjs provider over the engine RPC (59 §6.5): local updates go out as `doc_update`, the
 * `doc_subscribe` stream comes in as `Y.applyUpdate(doc, u, 'engine')`, awareness both ways.
 * The subscription is opened before `doc_open` so nothing accepted in between is missed; a
 * dropped connection or a `Closed{slow}` re-syncs from the state vector and flushes the queue.
 *
 * On a link device (`engine !== LOCAL`, 31 §7.3) the document and the FIFO of unacknowledged
 * local updates live in IndexedDB (§6.3): a killed app comes back with its offline edits and
 * replays them on the next `open`. In-process on the Mac the queue stays in memory.
 */
export class EngineProvider {
  readonly id: DocId
  readonly doc = new Y.Doc()
  readonly awareness: Awareness
  meta: DocMeta | null = null
  /** The engine serving the document and the other one during a partition (184 §10.3, §4.4); null before the first open / from an old server. */
  server: string | null = null
  dual: string | null = null
  status: DocStatus = { kind: 'connecting' }
  /** A viewer's provider (59 §8.1): the editor is `editable: false` (status `readonly`), awareness still flows. */
  readonly viewer: boolean
  #api: Api
  #client: RpcClient
  #store: DocStore | null
  /** IndexedDB read (state applied, FIFO loaded); local updates and `#sync` wait for it. */
  #ready: Promise<void>
  #loaded = false
  #connected = false
  /** Local updates the engine has not acknowledged, oldest first. */
  #queue: Pending[] = []
  /** Sent and acked, not yet covered by a server state vector (184 §4.7); memory only. */
  #unconfirmed: Pending[] = []
  #resent = new WeakSet<Pending>()
  #save: ReturnType<typeof setTimeout> | undefined
  /** Cancels the pending `#sync` retry, if any. */
  #cancelRetry = (): void => {}
  /** Failed `#sync`s in a row since the last good one. */
  #retries = 0
  /**
   * Something this client sent or holds may be missing on the server, so a catch-all with no structs (a lost
   * delete-only update) carries news. Kept until a catch-all lands: `#unconfirmed` is spent at the first attempt.
   */
  #owed = false
  #unsub: (() => Promise<void>) | null = null
  #offState: () => void
  #status = new Set<(s: DocStatus) => void>()
  #events = new Set<(e: DocEvent) => void>()
  #authored = new Set<(a: Authored) => void>()
  #destroyed = false
  /** Agent edits announced since this provider opened (59 §5.5: the page menu's undo is enabled by them). */
  agentEdits = 0

  /** `guest`: a share-link member's identity (59 §8.3) — pins the awareness `user`, and a viewer never goes `live`. */
  constructor(client: RpcClient, api: Api, id: DocId, engine: string = LOCAL, guest?: GuestUser) {
    this.#client = client
    this.#api = api
    this.id = id
    this.awareness = guest ? new PinnedUserAwareness(this.doc, guest) : new Awareness(this.doc)
    this.viewer = guest?.role === 'viewer'
    this.#store = engine === LOCAL ? null : new DocStore(engine, id)
    this.#loaded = this.#store === null
    this.#ready = this.#store ? this.#restore(this.#store) : Promise.resolve()
    this.doc.on('update', this.#local)
    this.awareness.on('update', this.#localAwareness)
    this.#offState = client.onDiscovery((discovery) => {
      const wasConnected = this.#connected
      const available = supports(discovery, { methods: this.viewer ? ['doc_open', 'doc_subscribe', 'doc_unsubscribe', 'doc_awareness'] : ['doc_open', 'doc_subscribe', 'doc_unsubscribe', 'doc_awareness', 'doc_update'] })
      this.#connected = ready(discovery) && (discovery.phase === 'legacy' || available)
      if (this.#connected && !wasConnected) void this.#sync()
      else if (discovery.phase === 'ready' && !available) { this.#cancelRetry(); this.#set({ kind: 'closed', reason: 'Unavailable on this engine' }) }
      else if (!this.#connected && (this.status.kind === 'live' || this.status.kind === 'readonly')) this.#set({ kind: 'offline', queued: this.#queue.length })
    })
  }

  /** Unacknowledged local updates (in memory; mirrored in IndexedDB on link devices). */
  get queued(): number {
    return this.#queue.length
  }

  onStatus(cb: (s: DocStatus) => void): () => void {
    this.#status.add(cb)
    return () => this.#status.delete(cb)
  }

  /** Every non-update event of the stream (`Exported`, `Snapshot`, file-origin `Update`) for the status line (§9.4). */
  onEvent(cb: (e: DocEvent) => void): () => void {
    this.#events.add(cb)
    return () => this.#events.delete(cb)
  }

  /** `Authored` events (agent-written blocks), delivered before the update that carries them. */
  onAuthored(cb: (a: Authored) => void): () => void {
    this.#authored.add(cb)
    return () => this.#authored.delete(cb)
  }

  /** The agent's presence, if some awareness state carries an `agent` field for this document. */
  agentPresence(): AgentPresence | null {
    for (const state of this.awareness.getStates().values()) {
      const agent = isAgentState(state) ? state.agent : null
      if (agent && agent.doc === this.id) {
        return { name: typeof state.name === 'string' ? state.name : 'theuth', color: typeof state.color === 'string' ? state.color : '#7c3aed', blocks: agent.blocks, at: agent.at }
      }
    }
    return null
  }

  destroy(): void {
    this.#destroyed = true
    this.#offState()
    this.doc.off('update', this.#local)
    this.awareness.off('update', this.#localAwareness)
    removeAwarenessStates(this.awareness, [this.doc.clientID], 'destroy')
    void this.#unsub?.()
    this.#unsub = null
    clearTimeout(this.#save)
    this.#cancelRetry()
    if (this.#store && this.#loaded) void this.#store.saveState(Y.encodeStateAsUpdate(this.doc))
    this.awareness.destroy()
    this.doc.destroy()
    this.#set({ kind: 'closed', reason: 'destroyed' })
  }

  async #restore(store: DocStore): Promise<void> {
    try {
      const { state, pending } = await store.load()
      if (this.#destroyed) return
      if (state) Y.applyUpdate(this.doc, state, RESTORE)
      this.#queue.unshift(...pending)
    } catch (e) {
      console.warn('doc store unavailable', this.id, e)
    }
    this.#loaded = true
    // Opened while the engine is unreachable: what IndexedDB had is editable now, not on the next reconnect.
    if (!this.#connected && this.status.kind === 'connecting') this.#set({ kind: 'offline', queued: this.#queue.length })
  }

  #scheduleSave(): void {
    if (!this.#store || this.#save) return
    this.#save = setTimeout(() => {
      this.#save = undefined
      if (!this.#destroyed) void this.#store?.saveState(Y.encodeStateAsUpdate(this.doc))
    }, SAVE_MS)
  }

  #set(s: DocStatus): void {
    this.status = s
    for (const cb of this.#status) cb(s)
  }

  #local = (update: Uint8Array, origin: unknown): void => {
    if (origin === RESTORE) return
    this.#scheduleSave()
    if (origin === ENGINE) return
    if (!this.#loaded) {
      // Nothing can be typed before the store is read (the editor is read-only while `connecting`); keep order anyway.
      void this.#ready.then(() => this.#local(update, origin))
      return
    }
    const p = this.#store ? this.#store.enqueue(update) : { seq: 0, update }
    if (this.status.kind !== 'live') {
      this.#queue.push(p)
      if (this.status.kind === 'offline') this.#set({ kind: 'offline', queued: this.#queue.length })
      return
    }
    this.#api
      .doc_update(this.id, b64(update))
      .then(() => this.#acked([p]))
      .catch((e: unknown) => {
        if (this.#refused(e, [p])) return
        this.#queue.push(p)
        this.#set({ kind: 'offline', queued: this.#queue.length })
      })
  }

  /** Resent entries are no longer in IndexedDB (acked at their first send); acking them again would skew its byte count. */
  #forget(items: Pending[]): Promise<void> | undefined {
    return this.#store?.ack(items.filter((p) => !this.#resent.has(p)))
  }

  /** Sent and acked: gone from the FIFO and IndexedDB, kept in memory until a server vector confirms it (184 §4.7, §13 row 36). */
  #acked(items: Pending[]): Promise<void> | undefined {
    this.#unconfirmed.push(...items)
    let bytes = this.#unconfirmed.reduce((n, p) => n + p.update.length, 0)
    // ponytail: 1 MiB cap, oldest dropped (a reload loses the list anyway; the catch-all push still covers it); raise if long partitions lose edits.
    while (bytes > MAX_UNCONFIRMED && this.#unconfirmed.length > 1) {
      bytes -= this.#unconfirmed.shift()?.update.length ?? 0
      console.warn('unconfirmed updates over 1 MiB; oldest dropped', this.id)
    }
    return this.#forget(items)
  }

  /**
   * A limit refusal (`UpdateTooLarge`, `DocTooLarge`): the update is dropped here and in IndexedDB and the
   * provider closes with the reason — this copy now holds text the engine never accepted, a reload shows
   * the engine's state. Not an offline condition: a retry would fail the same way forever.
   */
  #refused(e: unknown, dropped: Pending[]): boolean {
    const message = e instanceof Error ? e.message : String(e)
    const m = REFUSED.exec(message)
    if (!m) return false
    void this.#forget([...dropped, ...this.#queue.splice(0)])
    this.#set({ kind: 'closed', reason: m[1] === 'UpdateTooLarge' ? 'update too large (48 KiB); reload' : 'document full (8 MiB); reload' })
    return true
  }

  /** The longest prefix of the (non-empty) queue whose merged update fits under [`MAX_UPDATE_B64`] — at least one entry. */
  #batch(): { batch: Pending[]; merged: Uint8Array } {
    const [first, ...rest] = this.#queue
    if (!first) throw new Error('empty queue')
    const batch = [first]
    let merged = first.update
    // ponytail: re-merges per entry (O(n²) in queue length); fine for an offline FIFO of keystrokes.
    for (const p of rest) {
      const next = Y.mergeUpdates([merged, p.update])
      if (Math.ceil(next.length / 3) * 4 > MAX_UPDATE_B64) break
      merged = next
      batch.push(p)
    }
    return { batch, merged }
  }

  #localAwareness = (
    { added, updated, removed }: { added: number[]; updated: number[]; removed: number[] },
    origin: unknown,
  ): void => {
    if (origin === ENGINE || (this.status.kind !== 'live' && this.status.kind !== 'readonly')) return
    const changed = [...added, ...updated, ...removed]
    void this.#api.doc_awareness(this.id, b64(encodeAwarenessUpdate(this.awareness, changed))).catch(() => undefined)
  }

  #item = (e: DocEvent): void => {
    switch (e.type) {
      case 'Update':
        Y.applyUpdate(this.doc, unb64(e.update), ENGINE)
        if (e.by.who === 'file') for (const cb of this.#events) cb(e)
        return
      case 'Authored': {
        if (e.by.who === 'agent') this.agentEdits++
        const a: Authored = { blocks: e.blocks, by: e.by, at: Date.now() }
        for (const cb of this.#authored) cb(a)
        return
      }
      case 'Awareness':
        applyAwarenessUpdate(this.awareness, unb64(e.state), ENGINE)
        return
      case 'Meta':
        // A rename (59 §7.3): `meta` follows so the page chrome and `docRef` read the new title.
        if (this.meta) this.meta.title = e.title
        for (const cb of this.#events) cb(e)
        return
      case 'Closed':
        this.#unsub = null
        // `slow`: the engine dropped our queue; the state vector recovers what we missed. `hub_moved`,
        // `server_moved`, `NotHub:…`: the document's server changed (184 §4.7, §12.5); `#sync` re-subscribes and pushes.
        if (!this.#destroyed && (['slow', 'hub_moved', 'server_moved'].includes(e.reason) || e.reason.startsWith('NotHub:'))) {
          this.#set({ kind: 'connecting', resync: !this.viewer })
          void this.#sync()
        } else this.#set({ kind: 'closed', reason: e.reason })
        return
      case 'Server':
        this.server = e.server
        this.dual = e.dual
        for (const cb of this.#events) cb(e)
        return
      default:
        for (const cb of this.#events) cb(e)
    }
  }

  /** Subscribe (once), pull the missing state (the diff against our state vector, chunked), flush the FIFO. */
  async #sync(): Promise<void> {
    const generation = this.#client.generation
    if (this.#destroyed) return
    await this.#ready
    this.#cancelRetry()
    if (this.#destroyed || !this.#connected || generation !== this.#client.generation) return
    try {
      if (!this.#unsub) {
        this.#unsub = await this.#client.stream<DocEvent>('doc_subscribe', [this.id], 'doc_unsubscribe', this.#item)
      }
      const sv = b64(Y.encodeStateVector(this.doc))
      const opened = await this.#api.doc_open(this.id, sv)
      if (this.#destroyed || generation !== this.#client.generation) return
      this.meta = opened.meta
      this.server = opened.server ?? null
      this.dual = opened.dual ?? null
      // The chunks are slices of ONE v1 update (59 §4.7): joined, then applied as one transaction.
      const chunks = opened.state.map(unb64)
      const state = new Uint8Array(chunks.reduce((n, c) => n + c.length, 0))
      let at = 0
      for (const c of chunks) {
        state.set(c, at)
        at += c.length
      }
      if (state.length > 0) Y.applyUpdate(this.doc, state, ENGINE)
      if (opened.awareness !== '') applyAwarenessUpdate(this.awareness, unb64(opened.awareness), ENGINE)
      // 184 §4.7: an old server answers `sv: ''` — nothing below applies.
      const have = opened.sv ? Y.decodeStateVector(unb64(opened.sv)) : null
      // A viewer (or a read-only source doc) never writes: every push would be refused and counted.
      const readOnly = this.viewer || opened.meta.kind === 'source'
      this.#owed ||= this.#queue.length > 0 || this.#unconfirmed.length > 0
      if (have) {
        // Acked-but-unconfirmed updates the server's vector does not cover were lost behind its front door: resend first.
        const lost = this.#unconfirmed.filter((p) => !structs(p.update).every((s) => s.id.clock + s.length <= (have.get(s.id.client) ?? 0)) && !this.#queue.includes(p))
        this.#unconfirmed = []
        for (const p of lost) this.#resent.add(p)
        this.#queue.unshift(...lost)
        this.#owed ||= lost.length > 0
      }
      // Merged batches under the 48 KiB cap stand for everything typed while offline; another round picks
      // up what was typed while one was in flight, and what was typed during the catch-all, so nothing waits
      // for the next reconnect (the status turns live only with the queue empty, in the same tick).
      let catchAll = have !== null && !readOnly
      for (;;) {
        while (this.#queue.length > 0) {
          const { batch, merged } = this.#batch()
          this.#queue.splice(0, batch.length)
          if (this.status.kind === 'offline') this.#set({ kind: 'offline', queued: this.#queue.length })
          try {
            await this.#api.doc_update(this.id, b64(merged))
          } catch (e) {
            if (this.#refused(e, batch)) return
            this.#queue.unshift(...batch)
            throw e
          }
          // The server now holds these: advance its vector over every contiguous struct (a gap stays for the catch-all).
          if (have) for (const p of batch) for (const s of structs(p.update)) if (s.id.clock <= (have.get(s.id.client) ?? 0)) have.set(s.id.client, Math.max(have.get(s.id.client) ?? 0, s.id.clock + s.length))
          await this.#acked(batch)
        }
        if (catchAll && have) {
          catchAll = false
          // Catch-all (§12.5): what the server still lacks after the list and the queue. Yjs puts the whole delete
          // set in every update, so a remainder without structs is pushed only when something was owed.
          // Over the cap it is skipped: the list already covered the lost updates, and a push over the cap would trip `#refused`.
          const rest = Y.encodeStateAsUpdate(this.doc, svBytes(have))
          const news = rest.length > 2 && (this.#owed || Y.decodeUpdate(rest).structs.length > 0)
          if (news && Math.ceil(rest.length / 3) * 4 <= MAX_UPDATE_B64) {
            try {
              await this.#api.doc_update(this.id, b64(rest))
            } catch (e) {
              if (this.#refused(e, [])) return
              throw e
            }
          }
          this.#owed = false
        }
        if (this.#queue.length === 0) break
      }
      if (generation !== this.#client.generation) return
      this.#retries = 0
      this.#set(this.viewer ? { kind: 'readonly' } : { kind: 'live' })
    } catch (e) {
      if (this.#destroyed || generation !== this.#client.generation) return
      if (e instanceof UnsupportedCapability || e instanceof RpcError && e.code === -32601) {
        this.#cancelRetry()
        this.#set({ kind: 'closed', reason: 'Unavailable on this engine' })
        return
      }
      this.#set({ kind: 'offline', queued: this.#queue.length })
      console.warn('doc sync failed', this.id, e)
      // Z184-R2: an engine restarted under the page answers the first sync `NotHub{hub: null}` while its lease
      // warms up, and the socket stays up, so no reconnect retries it: the edits would sit queued for good.
      // Once a second for the first ten tries (a warm-up is ~30 s), then every 5-10 s while the page is open.
      if (this.#connected) {
        const delay = ++this.#retries > SYNC_RETRY_FAST ? 5000 + Math.random() * 5000 : SYNC_RETRY_MS
        const t = setTimeout(() => void this.#sync(), delay)
        this.#cancelRetry = () => clearTimeout(t)
      }
    }
  }
}

const providers = new Map<string, { provider: EngineProvider; refs: number }>()

/**
 * One provider per open document (§6.5): a document open in two panes shares it; `release` drops the last
 * reference. `engine` is the scope id (`'local'` or a link engine id, 31 §7.4) and keys the IndexedDB store.
 */
export function acquire(client: RpcClient, api: Api, id: DocId, engine: string = LOCAL): EngineProvider {
  let entry = providers.get(id)
  if (!entry) {
    entry = { provider: new EngineProvider(client, api, id, engine), refs: 0 }
    providers.set(id, entry)
  }
  entry.refs++
  return entry.provider
}

export function release(provider: EngineProvider): void {
  const entry = providers.get(provider.id)
  if (!entry || --entry.refs > 0) return
  providers.delete(provider.id)
  provider.destroy()
}
