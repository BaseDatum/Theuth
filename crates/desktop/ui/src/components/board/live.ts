// Design 71 §2.5 "Live" (M11/M12, reader side): one `code_locate` batch per board open — every `Excerpt` (with its
// text hash and `symbol`), every `code:` link and every diagram unit `ref` — and again on **Verify**. Results are
// cached by anchor for the life of the open board: a Yjs update locates only the anchors it added, the side pane
// asks for a stray one on demand (`ensure`), Verify drops the cache and asks for everything. The renderer never
// writes; the RPC is root-bounded (7a).
import { createContext, useContext } from 'react'
import { useStore } from 'zustand'
import { createStore, type StoreApi } from 'zustand/vanilla'
import type { Anchor } from '@/components/chat/surface/anchor'
import { excerptLines } from '@/components/chat/surface/parse'
import type { LocateAnchor } from '@/rpc/bindings/method.codeindex.input'
import type { Located } from "@/rpc/bindings/method.codeindex.output"
import { formatAnchor } from './anchors'
import { type DocBlock, type Inline, classifyCode, codeAnchor, excerptAnchor } from './blocks'
import { unitsOf } from './units'

export type LiveRequest = { key: string; anchor: LocateAnchor }

/** An `Excerpt` is its own anchor (its hash decides `changed`), keyed by block; links and refs by the anchor itself. */
export const excerptKey = (blockId: string) => `excerpt:${blockId}`

/** `text_sha` as `codeindex::locate::text_sha`: lowercase hex SHA-256 of the lines joined by `\n`. */
export async function sha256(text: string): Promise<string> {
  const d = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(text))
  return [...new Uint8Array(d)].map((b) => b.toString(16).padStart(2, '0')).join('')
}

export function toLocate(a: Anchor & { kind: 'code' }): LocateAnchor {
  return { path: a.path, start: a.lines?.[0] ?? null, end: a.lines?.[1] ?? null, symbol: null, text_sha: null }
}

/** Every inline run a block carries: prose content, and table cells. */
function* inlines(block: DocBlock): Generator<Inline> {
  const c = block.content as unknown
  if (Array.isArray(c)) yield* c as Inline[]
  else if (c && typeof c === 'object' && 'rows' in c) {
    for (const row of (c as { rows: { cells: unknown[] }[] }).rows) {
      for (const cell of row.cells) {
        const runs = Array.isArray(cell) ? cell : (cell as { content?: unknown[] }).content
        if (Array.isArray(runs)) yield* runs as Inline[]
      }
    }
  }
}

/** The batch for `blocks`, in document order, deduplicated: excerpts (hashed), `code:` links, unit `ref`s. */
export async function collectRequests(blocks: readonly DocBlock[]): Promise<LiveRequest[]> {
  const out: LiveRequest[] = []
  const seen = new Set<string>()
  const link = (a: Anchor | string | null) => {
    if (!a || typeof a === 'string' || a.kind !== 'code') return
    const key = formatAnchor(a)
    if (seen.has(key)) return
    seen.add(key)
    out.push({ key, anchor: toLocate(a) })
  }
  const walk = async (list: readonly DocBlock[]) => {
    for (const b of list) {
      if (b.type === 'codeBlock') {
        const c = classifyCode(b)
        if (c.kind === 'excerpt') {
          const a = excerptAnchor(c.props)
          const text_sha = await sha256(excerptLines(c.props.text).join('\n'))
          out.push({ key: excerptKey(b.id), anchor: { ...toLocate(a), symbol: c.props.symbol ?? null, text_sha } })
        } else if (c.kind === 'diagram') {
          for (const u of unitsOf(c.diagram)) link(u.anchor)
        }
      } else {
        for (const run of inlines(b)) if (run.type === 'link') link(codeAnchor(run.href))
      }
      if (b.children?.length) await walk(b.children as DocBlock[])
    }
  }
  await walk(blocks)
  return out
}

export type LiveState = {
  results: Map<string, Located>
  /** The whole batch failed (root not a git top-level, a path escaping it, codeindex disabled). */
  error: string | null
  /** Batches in flight. */
  pending: number
  verifiedAt: number | null
}

export type LiveStore = LiveState & {
  /** Locate the requests not yet cached (open, a Yjs insert, a stray pin). */
  ensure(reqs: readonly LiveRequest[]): void
  /** M11 Verify: forget everything and locate `reqs` again. */
  verify(reqs: readonly LiveRequest[]): void
}

export type Locate = (anchors: LocateAnchor[]) => Promise<Located[]>

export function createLiveStore(locate: Locate): StoreApi<LiveStore> {
  const inflight = new Set<string>()
  let gen = 0
  return createStore<LiveStore>()((set, get) => {
    const run = (reqs: readonly LiveRequest[]) => {
      const todo = reqs.filter((r) => !inflight.has(r.key))
      if (todo.length === 0) return
      for (const r of todo) inflight.add(r.key)
      const mine = gen
      set((s) => ({ pending: s.pending + 1 }))
      locate(todo.map((r) => r.anchor))
        .then(
          (rows) => {
            if (mine !== gen) return // a Verify superseded this batch
            const results = new Map(get().results)
            todo.forEach((r, i) => rows[i] && results.set(r.key, rows[i]))
            set({ results, error: null, verifiedAt: Date.now() })
          },
          (e: unknown) => mine === gen && set({ error: e instanceof Error ? e.message : String(e) }),
        )
        .finally(() => {
          todo.forEach((r) => inflight.delete(r.key))
          set((s) => ({ pending: s.pending - 1 }))
        })
    }
    return {
      results: new Map(),
      error: null,
      pending: 0,
      verifiedAt: null,
      ensure: (reqs) => run(reqs.filter((r) => !get().results.has(r.key))),
      verify(reqs) {
        gen++
        inflight.clear()
        set({ results: new Map(), error: null })
        run(reqs)
      },
    }
  })
}

/** M12 header: anchors whose status is not `same`. */
export function staleOf(results: Map<string, Located>, reqs: readonly LiveRequest[]): number {
  return reqs.filter((r) => (results.get(r.key)?.status ?? 'same') !== 'same').length
}

export const LiveContext = createContext<StoreApi<LiveStore> | null>(null)

export function useLiveStore(): StoreApi<LiveStore> {
  const s = useContext(LiveContext)
  if (!s) throw new Error('useLiveStore outside a BoardReader')
  return s
}

/** The live result for one key (`excerptKey(blockId)` or `formatAnchor(anchor)`); re-renders only when it changes. */
export function useLocated(key: string): Located | undefined {
  return useStore(useLiveStore(), (s) => s.results.get(key))
}

/** The chip text (M11): nothing for `same`, `changed`, `moved → L88–L101`, `missing`. */
export function chipText(loc: Located | undefined): string | null {
  if (!loc || loc.status === 'same') return null
  if (loc.status === 'moved') return `moved → L${loc.start}${loc.end > loc.start ? `–L${loc.end}` : ''}`
  return loc.status
}

/** A shiki language guess for the live window: the file extension (`rs`, `ts`, `py`… are shiki aliases). */
export function langOf(path: string): string | undefined {
  const dot = path.lastIndexOf('.')
  return dot > path.lastIndexOf('/') ? path.slice(dot + 1) : undefined
}
