import { Awareness, applyAwarenessUpdate, encodeAwarenessUpdate, removeAwarenessStates } from 'y-protocols/awareness'
import * as Y from 'yjs'
import { LOCAL } from '@/link/harnesses'
import type { Api } from '@/rpc/api'
import type { DocEvent } from '@/rpc/bindings/DocEvent'
import type { DocId } from '@/rpc/bindings/DocId'
import type { DocMeta } from '@/rpc/bindings/DocMeta'
import type { Who } from '@/rpc/bindings/Who'
import type { RpcClient } from '@/rpc/client'
import { DocStore, type Pending } from './queue'

/** Origin tag of every change the harness sent us; never echoed back. */
const HARNESS = 'harness'
/** Origin of the IndexedDB state applied at start: already ours, already (or pending to be) the harness's. */
const RESTORE = 'restore'
/** The whole Y.Doc is written to IndexedDB this long after the last change (link devices only). */
const SAVE_MS = 1000

export type DocStatus =
  | { kind: 'connecting' }
  | { kind: 'live' }
  | { kind: 'offline'; queued: number }
  | { kind: 'readonly' }
  | { kind: 'closed'; reason: string }

/** Who wrote a set of blocks, from a `DocEvent::Authored` (59 §5.5). */
export type Authored = { blocks: string[]; by: Who; at: number }

/** The agent's awareness state while it edits (59 §6.4, §8.5): `{ agent: { doc, blocks, at } }` plus `name`/`color`. */
export type AgentPresence = { name: string; color: string; blocks: string[]; at: number }

type AgentState = { name?: unknown; color?: unknown; agent: { doc: string; blocks: string[]; at: number } }

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

function b64(bytes: Uint8Array): string {
  // Spreading a multi-MiB state into `fromCharCode` overflows the argument limit.
  let s = ''
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000))
  return btoa(s)
}
const unb64 = (s: string): Uint8Array => Uint8Array.from(atob(s), (c) => c.charCodeAt(0))

/**
 * A Yjs provider over the harness RPC (59 §6.5): local updates go out as `doc_update`, the
 * `doc_subscribe` stream comes in as `Y.applyUpdate(doc, u, 'harness')`, awareness both ways.
 * The subscription is opened before `doc_open` so nothing accepted in between is missed; a
 * dropped connection or a `Closed{slow}` re-syncs from the state vector and flushes the queue.
 *
 * On a link device (`harness !== LOCAL`, 31 §7.3) the document and the FIFO of unacknowledged
 * local updates live in IndexedDB (§6.3): a killed app comes back with its offline edits and
 * replays them on the next `open`. In-process on the Mac the queue stays in memory.
 */
export class HarnessProvider {
  readonly id: DocId
  readonly doc = new Y.Doc()
  readonly awareness = new Awareness(this.doc)
  meta: DocMeta | null = null
  status: DocStatus = { kind: 'connecting' }
  #api: Api
  #client: RpcClient
  #store: DocStore | null
  /** IndexedDB read (state applied, FIFO loaded); local updates and `#sync` wait for it. */
  #ready: Promise<void>
  #loaded = false
  #connected = false
  /** Local updates the harness has not acknowledged, oldest first. */
  #queue: Pending[] = []
  #save: ReturnType<typeof setTimeout> | undefined
  #unsub: (() => Promise<void>) | null = null
  #offState: () => void
  #status = new Set<(s: DocStatus) => void>()
  #events = new Set<(e: DocEvent) => void>()
  #authored = new Set<(a: Authored) => void>()
  #destroyed = false
  /** Agent edits announced since this provider opened (59 §5.5: the page menu's undo is enabled by them). */
  agentEdits = 0

  constructor(client: RpcClient, api: Api, id: DocId, harness: string = LOCAL) {
    this.#client = client
    this.#api = api
    this.id = id
    this.#store = harness === LOCAL ? null : new DocStore(harness, id)
    this.#loaded = this.#store === null
    this.#ready = this.#store ? this.#restore(this.#store) : Promise.resolve()
    this.doc.on('update', this.#local)
    this.awareness.on('update', this.#localAwareness)
    this.#offState = client.onState((s) => {
      this.#connected = s === 'open'
      if (s === 'open') void this.#sync()
      else if (this.status.kind === 'live') this.#set({ kind: 'offline', queued: this.#queue.length })
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
    // Opened while the harness is unreachable: what IndexedDB had is editable now, not on the next reconnect.
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
    if (origin === HARNESS) return
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
      .then(() => this.#store?.ack([p]))
      .catch(() => {
        this.#queue.push(p)
        this.#set({ kind: 'offline', queued: this.#queue.length })
      })
  }

  #localAwareness = (
    { added, updated, removed }: { added: number[]; updated: number[]; removed: number[] },
    origin: unknown,
  ): void => {
    if (origin === HARNESS || this.status.kind !== 'live') return
    const changed = [...added, ...updated, ...removed]
    void this.#api.doc_awareness(this.id, b64(encodeAwarenessUpdate(this.awareness, changed))).catch(() => undefined)
  }

  #item = (e: DocEvent): void => {
    switch (e.type) {
      case 'Update':
        Y.applyUpdate(this.doc, unb64(e.update), HARNESS)
        if (e.by.who === 'file') for (const cb of this.#events) cb(e)
        return
      case 'Authored': {
        if (e.by.who === 'agent') this.agentEdits++
        const a: Authored = { blocks: e.blocks, by: e.by, at: Date.now() }
        for (const cb of this.#authored) cb(a)
        return
      }
      case 'Awareness':
        applyAwarenessUpdate(this.awareness, unb64(e.state), HARNESS)
        return
      case 'Closed':
        this.#unsub = null
        // `slow`: the harness dropped our queue; the state vector recovers what we missed.
        if (e.reason === 'slow' && !this.#destroyed) void this.#sync()
        else this.#set({ kind: 'closed', reason: e.reason })
        return
      default:
        for (const cb of this.#events) cb(e)
    }
  }

  /** Subscribe (once), pull the missing state (the diff against our state vector, chunked), flush the FIFO. */
  async #sync(): Promise<void> {
    if (this.#destroyed) return
    await this.#ready
    if (this.#destroyed || !this.#connected) return
    try {
      if (!this.#unsub) {
        this.#unsub = await this.#client.subscribeTo<DocEvent>('doc_subscribe', 'doc_unsubscribe', [this.id], this.#item)
      }
      const sv = b64(Y.encodeStateVector(this.doc))
      const opened = await this.#api.doc_open(this.id, sv)
      if (this.#destroyed) return
      this.meta = opened.meta
      // The chunks are slices of ONE v1 update (59 §4.7): joined, then applied as one transaction.
      const chunks = opened.state.map(unb64)
      const state = new Uint8Array(chunks.reduce((n, c) => n + c.length, 0))
      let at = 0
      for (const c of chunks) {
        state.set(c, at)
        at += c.length
      }
      if (state.length > 0) Y.applyUpdate(this.doc, state, HARNESS)
      if (opened.awareness !== '') applyAwarenessUpdate(this.awareness, unb64(opened.awareness), HARNESS)
      // One merged update per round stands for everything typed while offline; another round picks up
      // what was typed while that one was in flight, so nothing waits for the next reconnect.
      while (this.#queue.length > 0) {
        const batch = this.#queue.splice(0)
        if (this.status.kind === 'offline') this.#set({ kind: 'offline', queued: 0 })
        try {
          await this.#api.doc_update(this.id, b64(Y.mergeUpdates(batch.map((p) => p.update))))
        } catch (e) {
          this.#queue.unshift(...batch)
          throw e
        }
        await this.#store?.ack(batch)
      }
      this.#set({ kind: 'live' })
    } catch (e) {
      if (this.#destroyed) return
      this.#set({ kind: 'offline', queued: this.#queue.length })
      console.warn('doc sync failed', this.id, e)
    }
  }
}

const providers = new Map<string, { provider: HarnessProvider; refs: number }>()

/**
 * One provider per open document (§6.5): a document open in two panes shares it; `release` drops the last
 * reference. `harness` is the scope id (`'local'` or a link harness id, 31 §7.4) and keys the IndexedDB store.
 */
export function acquire(client: RpcClient, api: Api, id: DocId, harness: string = LOCAL): HarnessProvider {
  let entry = providers.get(id)
  if (!entry) {
    entry = { provider: new HarnessProvider(client, api, id, harness), refs: 0 }
    providers.set(id, entry)
  }
  entry.refs++
  return entry.provider
}

export function release(provider: HarnessProvider): void {
  const entry = providers.get(provider.id)
  if (!entry || --entry.refs > 0) return
  providers.delete(provider.id)
  provider.destroy()
}
