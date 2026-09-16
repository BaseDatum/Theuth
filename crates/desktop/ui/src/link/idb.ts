// Raw IndexedDB, promisified: one db `link` with the `identity` and `harnesses`
// object stores (31 §7.2). Nothing here touches localStorage/sessionStorage.

const DB = 'link'
const STORES = ['identity', 'harnesses'] as const
export type Store = (typeof STORES)[number]

function req<T>(r: IDBRequest<T>): Promise<T> {
  const { promise, resolve, reject } = Promise.withResolvers<T>()
  r.onsuccess = () => resolve(r.result)
  r.onerror = () => reject(r.error)
  return promise
}

let db: Promise<IDBDatabase> | undefined

/** Open (and on first use create) the `link` database; cached. */
export function linkDb(): Promise<IDBDatabase> {
  if (!db) {
    const open = indexedDB.open(DB, 1)
    open.onupgradeneeded = () => {
      for (const s of STORES) if (!open.result.objectStoreNames.contains(s)) open.result.createObjectStore(s)
    }
    db = req(open)
  }
  return db
}

export async function idbGet<T>(store: Store, key: string): Promise<T | undefined> {
  const d = await linkDb()
  return req<T | undefined>(d.transaction(store).objectStore(store).get(key))
}

export async function idbPut(store: Store, key: string, value: unknown): Promise<void> {
  const d = await linkDb()
  const tx = d.transaction(store, 'readwrite')
  tx.objectStore(store).put(value, key)
  const { promise, resolve, reject } = Promise.withResolvers<void>()
  tx.oncomplete = () => resolve()
  tx.onerror = tx.onabort = () => reject(tx.error)
  return promise
}
