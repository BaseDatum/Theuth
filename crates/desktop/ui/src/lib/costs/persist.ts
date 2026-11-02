// 160 §6.2: the merged cube on disk for a cold start — IndexedDB db `theuth-costs`, store `cube`, key `v1:<engine>`.
// `link/idb.ts` is bound to the `link` db and its two stores, so this is its own small wrapper.

import type { CostCube } from '@/rpc/bindings/CostCube'

let db: Promise<IDBDatabase> | undefined

function open(): Promise<IDBDatabase> {
  if (!db) {
    const { promise, resolve, reject } = Promise.withResolvers<IDBDatabase>()
    const r = indexedDB.open('theuth-costs', 1)
    r.onupgradeneeded = () => r.result.createObjectStore('cube')
    r.onsuccess = () => resolve(r.result)
    r.onerror = () => reject(r.error)
    db = promise
  }
  return db
}

export const cubeKey = (engine: string): string => `v1:${engine}`

/** The stored cube for `engine`, or `undefined` (none yet, or IndexedDB unavailable). */
export async function loadCube(engine: string): Promise<CostCube | undefined> {
  try {
    const d = await open()
    const r = d.transaction('cube').objectStore('cube').get(cubeKey(engine))
    const { promise, resolve, reject } = Promise.withResolvers<CostCube | undefined>()
    r.onsuccess = () => resolve(r.result as CostCube | undefined)
    r.onerror = () => reject(r.error)
    return await promise
  } catch {
    return undefined
  }
}

export async function saveCube(engine: string, cube: CostCube): Promise<void> {
  const d = await open()
  const tx = d.transaction('cube', 'readwrite')
  tx.objectStore('cube').put(cube, cubeKey(engine))
  const { promise, resolve, reject } = Promise.withResolvers<void>()
  tx.oncomplete = () => resolve()
  tx.onerror = tx.onabort = () => reject(tx.error)
  return promise
}
