// 144 §5.1 (writer's call W-g): the dial lives on the session server-side; this device remembers the last position
// per (notebook, item) in `localStorage` (`note-chat.ts`'s pattern) and the chat column applies it to a NEW thread
// with `notebook_context_set` right after the session exists. Without a session the dial writes only here.
// §5.2: the pure words of the context line (`in context: 4 sources (2 summary · 1 full · 1 mention) · 2 notes ·
// ~12.4k of 40k tokens · 1 omitted`) live here too, shared by the chat column and the bulk rows.
import { create } from 'zustand'
import type { ContextEstimate } from '@/rpc/bindings/ContextEstimate'
import type { ContextLevel } from '@/rpc/bindings/ContextLevel'
import type { ItemKind } from '@/rpc/bindings/ItemKind'

/** The estimate's item, named structurally: the Rust `ContextItem` is exported under a TS name the closer decides. */
export type EstimateItem = ContextEstimate['items'][number]

/** `12.4k`, `940`, `40k`. */
export const fmtTokens = (n: number): string => (n < 1000 ? String(n) : `${(n / 1000).toFixed(1).replace(/\.0$/, '')}k`)

/** `4 sources (2 summary · 1 full · 1 mention)` — only the levels present, full → summary → mention, `off` never counted. */
export function kindWords(items: readonly EstimateItem[], kind: ItemKind, word: string): string | null {
  const on = items.filter((i) => i.kind === kind && i.level !== 'off')
  if (on.length === 0) return null
  const counts = (['summary', 'full', 'mention'] as const).map((l) => [on.filter((i) => i.level === l).length, l] as const).filter(([n]) => n > 0)
  const breakdown = counts.length > 1 ? ` (${counts.map(([n, l]) => `${n} ${l}`).join(' · ')})` : counts.length === 1 && counts[0]![1] !== 'full' ? ` (${counts[0]![1]})` : ''
  return `${on.length} ${word}${on.length === 1 ? '' : 's'}${breakdown}`
}

/** The context line's parts (§5.2); `empty` → the one sentence. */
export function contextLine(e: ContextEstimate): string[] {
  if (e.empty) return ['nothing in context — the agent sees only the description']
  const parts = [kindWords(e.items, 'source', 'source'), kindWords(e.items, 'note', 'note')].filter((p): p is string => p !== null)
  parts.push(`~${fmtTokens(e.total_tokens)} of ${fmtTokens(e.budget_tokens)} tokens`)
  if (e.omitted > 0) parts.push(`${e.omitted} omitted`)
  return parts
}

const KEY = 'notebook-context'

export type Remembered = { kind: ItemKind; level: ContextLevel }
/** notebook → item → the last dial position. */
export type Saved = Record<string, Record<string, Remembered>>

type Store = {
  defaults: Saved
  set(notebook: string, item: string, kind: ItemKind, level: ContextLevel): void
  clear(notebook: string, item: string): void
  /** The bulk row: every item of `kind` the column shows (a bulk set is remembered per item, like single sets). */
  setAll(notebook: string, items: readonly { id: string; kind: ItemKind }[], level: ContextLevel): void
}

function load(): Saved {
  try {
    return JSON.parse(globalThis.localStorage?.getItem(KEY) ?? '{}') as Saved
  } catch {
    return {}
  }
}

const save = (defaults: Saved) => globalThis.localStorage?.setItem(KEY, JSON.stringify(defaults))

export const useContextDefaults = create<Store>()((set, get) => ({
  defaults: load(),
  set(notebook, item, kind, level) {
    const defaults = { ...get().defaults, [notebook]: { ...get().defaults[notebook], [item]: { kind, level } } }
    save(defaults)
    set({ defaults })
  },
  clear(notebook, item) {
    const { [item]: _, ...rest } = get().defaults[notebook] ?? {}
    const defaults = { ...get().defaults, [notebook]: rest }
    save(defaults)
    set({ defaults })
  },
  setAll(notebook, items, level) {
    const nb = { ...get().defaults[notebook] }
    for (const i of items) nb[i.id] = { kind: i.kind, level }
    const defaults = { ...get().defaults, [notebook]: nb }
    save(defaults)
    set({ defaults })
  },
}))

/** Pure: the remembered items of one notebook, for the `notebook_context_set` calls on a new thread. */
export const remembered = (saved: Saved, notebook: string): (Remembered & { item: string })[] =>
  Object.entries(saved[notebook] ?? {}).map(([item, r]) => ({ item, ...r }))
