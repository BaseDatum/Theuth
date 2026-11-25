// §8.3 `useBoard`: the seven maps as one plain Board, structurally shared — an entry nobody touched keeps its object
// identity, so memoised cards and ink paths do not re-render or re-tessellate.
import { useSyncExternalStore } from 'react'
import type * as Y from 'yjs'
import type { EngineProvider } from '@/lib/doc/provider'
import {
  indexMarks, mapsOf, readBoard, readCard, readFrame, readLink, readMark, readMeta, readStroke,
  type Board, type Id,
} from './model'

type EntryMap = 'cards' | 'links' | 'frames' | 'ink' | 'marks'
const READERS = { cards: readCard, links: readLink, frames: readFrame, ink: readStroke, marks: readMark } as const

export type BoardSource = { get(): Board; subscribe(cb: () => void): () => void; destroy(): void }

/** A live Board over a Y.Doc; re-reads only the entries a transaction touched. */
export function boardSource(doc: Y.Doc): BoardSource {
  const maps = mapsOf(doc)
  let board = readBoard(doc)
  const dirty: Record<EntryMap | 'meta' | 'aliases', Set<string> | true | null> = {
    meta: null, cards: null, links: null, frames: null, ink: null, marks: null, aliases: null,
  }
  const listeners = new Set<() => void>()
  const offs: (() => void)[] = []
  for (const name of ['meta', 'cards', 'links', 'frames', 'ink', 'marks', 'aliases'] as const) {
    const map = maps[name]
    const fn = (events: Y.YEvent<Y.AbstractType<unknown>>[]) => {
      const cur = dirty[name]
      if (cur === true) return
      const set = cur ?? new Set<string>()
      for (const e of events) {
        if (e.target === map) for (const k of (e as Y.YMapEvent<unknown>).keysChanged) set.add(k)
        else if (typeof e.path[0] === 'string') set.add(e.path[0])
        else {
          dirty[name] = true
          return
        }
      }
      dirty[name] = set
    }
    map.observeDeep(fn)
    offs.push(() => map.unobserveDeep(fn))
  }
  const flush = () => {
    if (Object.values(dirty).every((d) => d === null)) return
    const next: Board = { ...board }
    if (dirty.meta) next.meta = readMeta(maps.meta)
    if (dirty.aliases) {
      const a: Record<Id, string> = {}
      for (const [k, v] of maps.aliases.entries()) if (typeof v === 'string') a[k] = v
      next.aliases = a
    }
    for (const name of ['cards', 'links', 'frames', 'ink', 'marks'] as const) {
      const d = dirty[name]
      if (!d) continue
      const read = READERS[name] as (id: Id, v: unknown) => unknown
      const rec: Record<Id, unknown> = { ...(board[name] as Record<Id, unknown>) }
      const keys = d === true ? new Set([...Object.keys(rec), ...maps[name].keys()]) : d
      for (const k of keys) {
        const v = maps[name].get(k)
        const x = v === undefined ? null : read(k, v)
        if (x) rec[k] = x
        else delete rec[k]
      }
      ;(next as Record<string, unknown>)[name] = rec
    }
    if (dirty.marks) next.markOf = indexMarks(next.marks)
    for (const k of Object.keys(dirty) as (keyof typeof dirty)[]) dirty[k] = null
    board = next
    for (const l of listeners) l()
  }
  doc.on('afterTransaction', flush)
  offs.push(() => doc.off('afterTransaction', flush))
  return {
    get: () => board,
    subscribe(cb) {
      listeners.add(cb)
      return () => listeners.delete(cb)
    },
    destroy: () => offs.forEach((f) => f()),
  }
}

const sources = new WeakMap<Y.Doc, BoardSource>()

/** One shared source per document (several views of one whiteboard share it; it lives as long as the doc). */
export function sourceOf(doc: Y.Doc): BoardSource {
  let s = sources.get(doc)
  if (!s) {
    s = boardSource(doc)
    sources.set(doc, s)
  }
  return s
}

export function useBoard(provider: EngineProvider): Board {
  const src = sourceOf(provider.doc)
  return useSyncExternalStore(src.subscribe, src.get, src.get)
}
