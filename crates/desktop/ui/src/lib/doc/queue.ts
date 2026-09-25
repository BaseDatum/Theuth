// Per-(engine, document) offline store for link devices (59 §6.3, 31 §0.8: IndexedDB, never
// localStorage). Two object stores in db `documents`: `state` holds the whole Y.Doc as one
// `encodeStateAsUpdate` blob per document; `pending` holds the FIFO of local updates the engine
// has not acknowledged, one row per update keyed `[engine, doc, seq]` so a single row is written
// per keystroke and deleted per ack.

const DB = 'documents'
const STATE = 'state'
const PENDING = 'pending'
/** Per document: state blob plus pending rows. Beyond it the state blob is dropped (the engine has it); pending never is. */
export const MAX_BYTES = 4 * 1024 * 1024

export type Pending = { seq: number; update: Uint8Array }

function req<T>(r: IDBRequest<T>): Promise<T> {
  const { promise, resolve, reject } = Promise.withResolvers<T>()
  r.onsuccess = () => resolve(r.result)
  r.onerror = () => reject(r.error)
  return promise
}

function done(tx: IDBTransaction): Promise<void> {
  const { promise, resolve, reject } = Promise.withResolvers<void>()
  tx.oncomplete = () => resolve()
  tx.onerror = tx.onabort = () => reject(tx.error)
  return promise
}

let db: Promise<IDBDatabase> | undefined

function open(): Promise<IDBDatabase> {
  if (!db) {
    const o = indexedDB.open(DB, 1)
    o.onupgradeneeded = () => {
      for (const s of [STATE, PENDING]) if (!o.result.objectStoreNames.contains(s)) o.result.createObjectStore(s)
    }
    db = req(o)
  }
  return db
}

/** Tests swap the IndexedDB implementation; forget the cached handle. */
export function resetDb(): void {
  db = undefined
}

export class DocStore {
  readonly #key: string
  readonly #range: IDBKeyRange
  readonly #engine: string
  readonly #doc: string
  /** Row keys: `Date.now() × 4096` at construction, then +1 per update — two tabs on the same document (one origin,
   *  one IndexedDB) never write the same key, and rows from an earlier run sort before this run's. */
  #seq = Date.now() * 4096
  #pendingBytes = 0

  constructor(engine: string, doc: string) {
    this.#engine = engine
    this.#doc = doc
    this.#key = `${engine}/${doc}`
    this.#range = IDBKeyRange.bound([engine, doc, 0], [engine, doc, Number.MAX_SAFE_INTEGER])
  }

  /** The persisted state (if any) and the pending FIFO in seq order; later `enqueue`s sort after every loaded row. */
  async load(): Promise<{ state: Uint8Array | null; pending: Pending[] }> {
    const d = await open()
    const tx = d.transaction([STATE, PENDING])
    const [state, keys, values] = await Promise.all([
      req<Uint8Array | undefined>(tx.objectStore(STATE).get(this.#key)),
      req<IDBValidKey[]>(tx.objectStore(PENDING).getAllKeys(this.#range)),
      req<Uint8Array[]>(tx.objectStore(PENDING).getAll(this.#range)),
    ])
    const pending = keys.map((k, i) => ({ seq: (k as [string, string, number])[2], update: values[i] as Uint8Array }))
    this.#seq = Math.max(this.#seq, (pending.at(-1)?.seq ?? -1) + 1)
    this.#pendingBytes = pending.reduce((n, p) => n + p.update.length, 0)
    return { state: state ?? null, pending }
  }

  /** Append one local update; the seq is assigned synchronously so callers keep FIFO order without awaiting. */
  enqueue(update: Uint8Array): Pending {
    const p = { seq: this.#seq++, update }
    this.#pendingBytes += update.length
    void open().then((d) => {
      const tx = d.transaction(PENDING, 'readwrite')
      tx.objectStore(PENDING).put(update, [this.#engine, this.#doc, p.seq])
      return done(tx)
    })
    return p
  }

  /** The engine applied these: forget them. */
  async ack(items: Pending[]): Promise<void> {
    if (items.length === 0) return
    for (const p of items) this.#pendingBytes -= p.update.length
    const d = await open()
    const tx = d.transaction(PENDING, 'readwrite')
    for (const p of items) tx.objectStore(PENDING).delete([this.#engine, this.#doc, p.seq])
    await done(tx)
  }

  /** Persist the whole document; over `MAX_BYTES` (with pending) the blob is dropped instead and a warning logged. */
  async saveState(state: Uint8Array): Promise<void> {
    const d = await open()
    const tx = d.transaction(STATE, 'readwrite')
    if (state.length + this.#pendingBytes > MAX_BYTES) {
      console.warn(`doc ${this.#key}: ${state.length + this.#pendingBytes} bytes exceed the ${MAX_BYTES} IndexedDB bound; state dropped, pending kept`)
      tx.objectStore(STATE).delete(this.#key)
    } else {
      tx.objectStore(STATE).put(state, this.#key)
    }
    await done(tx)
  }
}
