import { Awareness, applyAwarenessUpdate, encodeAwarenessUpdate, removeAwarenessStates } from 'y-protocols/awareness'
import * as Y from 'yjs'
import type { Api } from '@/rpc/api'
import type { DocEvent } from '@/rpc/bindings/DocEvent'
import type { DocId } from '@/rpc/bindings/DocId'
import type { DocMeta } from '@/rpc/bindings/DocMeta'
import type { RpcClient } from '@/rpc/client'

/** Origin tag of every change the harness sent us; never echoed back. */
const HARNESS = 'harness'

export type DocStatus =
  | { kind: 'connecting' }
  | { kind: 'live' }
  | { kind: 'offline'; queued: number }
  | { kind: 'readonly' }
  | { kind: 'closed'; reason: string }

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
 */
export class HarnessProvider {
  readonly id: DocId
  readonly doc = new Y.Doc()
  readonly awareness = new Awareness(this.doc)
  meta: DocMeta | null = null
  status: DocStatus = { kind: 'connecting' }
  #api: Api
  #client: RpcClient
  // TODO(59 step 4): persist the queue in IndexedDB so a killed app does not lose offline edits.
  #queue: Uint8Array[] = []
  #unsub: (() => Promise<void>) | null = null
  #offState: () => void
  #status = new Set<(s: DocStatus) => void>()
  #events = new Set<(e: DocEvent) => void>()
  #destroyed = false

  constructor(client: RpcClient, api: Api, id: DocId) {
    this.#client = client
    this.#api = api
    this.id = id
    this.doc.on('update', this.#local)
    this.awareness.on('update', this.#localAwareness)
    this.#offState = client.onState((s) => {
      if (s === 'open') void this.#sync()
      else if (this.status.kind === 'live') this.#set({ kind: 'offline', queued: this.#queue.length })
    })
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

  destroy(): void {
    this.#destroyed = true
    this.#offState()
    this.doc.off('update', this.#local)
    this.awareness.off('update', this.#localAwareness)
    removeAwarenessStates(this.awareness, [this.doc.clientID], 'destroy')
    void this.#unsub?.()
    this.#unsub = null
    this.awareness.destroy()
    this.doc.destroy()
    this.#set({ kind: 'closed', reason: 'destroyed' })
  }

  #set(s: DocStatus): void {
    this.status = s
    for (const cb of this.#status) cb(s)
  }

  #local = (update: Uint8Array, origin: unknown): void => {
    if (origin === HARNESS) return
    if (this.status.kind !== 'live') {
      this.#queue.push(update)
      if (this.status.kind === 'offline') this.#set({ kind: 'offline', queued: this.#queue.length })
      return
    }
    this.#api.doc_update(this.id, b64(update)).catch(() => {
      this.#queue.push(update)
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

  /** Subscribe (once), pull the missing state, flush queued local updates. */
  async #sync(): Promise<void> {
    if (this.#destroyed) return
    try {
      if (!this.#unsub) {
        this.#unsub = await this.#client.subscribeTo<DocEvent>('doc_subscribe', 'doc_unsubscribe', [this.id], this.#item)
      }
      const sv = b64(Y.encodeStateVector(this.doc))
      const opened = await this.#api.doc_open(this.id, sv)
      if (this.#destroyed) return
      this.meta = opened.meta
      const chunks = opened.state.map(unb64)
      const state = new Uint8Array(chunks.reduce((n, c) => n + c.length, 0))
      let at = 0
      for (const c of chunks) {
        state.set(c, at)
        at += c.length
      }
      if (state.length > 0) Y.applyUpdate(this.doc, state, HARNESS)
      if (opened.awareness !== '') applyAwarenessUpdate(this.awareness, unb64(opened.awareness), HARNESS)
      // One merged update stands for everything typed while offline.
      const queued = this.#queue.splice(0)
      if (queued.length > 0) await this.#api.doc_update(this.id, b64(Y.mergeUpdates(queued)))
      this.#set({ kind: 'live' })
    } catch (e) {
      if (this.#destroyed) return
      this.#set({ kind: 'offline', queued: this.#queue.length })
      console.warn('doc sync failed', this.id, e)
    }
  }
}

const providers = new Map<string, { provider: HarnessProvider; refs: number }>()

/** One provider per open document (§6.5): a document open in two panes shares it; `release` drops the last reference. */
export function acquire(client: RpcClient, api: Api, id: DocId): HarnessProvider {
  let entry = providers.get(id)
  if (!entry) {
    entry = { provider: new HarnessProvider(client, api, id), refs: 0 }
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
