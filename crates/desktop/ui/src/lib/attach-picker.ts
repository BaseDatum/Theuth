// The attach picker's pure parts (66 §7.4, §7.7): tab → categories, browse merge/dedupe, recents, the client-side
// filter that stands in for the `notes`/`meetings` search categories until phase 5, and the keyboard map.

import { defaultFilter } from 'cmdk'
import { type ArtifactRecord, type AttachRef, type Kind, key, parse, toRef } from '@/lib/attach'
import type { SearchCategory } from '@/rpc/bindings/SearchCategory'

export type Tab = 'all' | Kind
export const TABS: readonly Tab[] = ['all', 'note', 'knowledge', 'artifact', 'session', 'meeting']
export const TAB_LABEL: Record<Tab, string> = {
  all: 'All',
  note: 'Notes',
  knowledge: 'Knowledge',
  artifact: 'Artifacts',
  session: 'Chats',
  meeting: 'Meetings',
}

/** Which `search` categories a tab requests; notes and meetings have none yet (phase 5) and filter client-side. */
export function categoriesFor(tab: Tab): SearchCategory[] {
  switch (tab) {
    case 'all':
      return ['sessions', 'artifacts', 'knowledge', 'knowledge_history']
    case 'knowledge':
      return ['knowledge', 'knowledge_history']
    case 'artifact':
      return ['artifacts']
    case 'session':
      return ['sessions']
    case 'note':
    case 'meeting':
      return []
  }
}

/** ⇥ / ⇧⇥: the next tab, wrapping. */
export function nextTab(tab: Tab, dir: 1 | -1): Tab {
  return TABS[(TABS.indexOf(tab) + dir + TABS.length) % TABS.length] ?? 'all'
}

/** Browse and tab row caps (§7.4): 5 per group under All, 30 under a tab. */
export const cap = (tab: Tab, kind: Kind, all = 5, own = 30): number => (tab === kind ? own : all)

/** The latest version per `(session, name)`, newest first — what the picker shows by default (§3.1). */
export function latestVersions<A extends ArtifactRecord>(artifacts: readonly A[]): A[] {
  const best = new Map<string, A>()
  for (const a of artifacts) {
    const k = `${a.session}\0${a.name}`
    const cur = best.get(k)
    if (!cur || a.version > cur.version) best.set(k, a)
  }
  return [...best.values()].sort((a, b) => Number(b.created_at) - Number(a.created_at))
}

/**
 * "In this chat" (§7.4): the session's artifacts (latest version per name) and the references parsed from its user
 * rows, newest first, deduped by `kind+id`, capped. `entries` are oldest first as the transcript keeps them.
 */
export function inThisChat(
  artifacts: readonly ArtifactRecord[],
  entries: readonly { kind: string; text?: string }[],
  limit = 6,
  now = new Date(),
): AttachRef[] {
  const out: AttachRef[] = []
  for (let i = entries.length - 1; i >= 0; i--) {
    const e = entries[i]
    if (e?.kind !== 'user' || e.text === undefined) continue
    for (const r of parse(e.text).refs.reverse()) out.push(r)
  }
  for (const a of latestVersions(artifacts)) out.push(toRef({ kind: 'artifact', artifact: a }, now))
  return dedupe(out).slice(0, limit)
}

/** First occurrence wins. */
export function dedupe(refs: readonly AttachRef[]): AttachRef[] {
  const seen = new Set<string>()
  return refs.filter((r) => {
    const k = key(r)
    if (seen.has(k)) return false
    seen.add(k)
    return true
  })
}

export const RECENTS_MAX = 20

/** Recents after a send: the sent references first (in order), then what was there, deduped, capped. */
export function pushRecents(recents: readonly AttachRef[], sent: readonly AttachRef[]): AttachRef[] {
  return dedupe([...sent, ...recents]).slice(0, RECENTS_MAX)
}

const recentsKey = (harness: string) => `attach-recents:${harness}`

export function loadRecents(harness: string): AttachRef[] {
  try {
    const raw = localStorage.getItem(recentsKey(harness))
    return raw ? (JSON.parse(raw) as AttachRef[]) : []
  } catch {
    return []
  }
}

export function saveRecents(harness: string, refs: readonly AttachRef[]): void {
  try {
    localStorage.setItem(recentsKey(harness), JSON.stringify(refs))
  } catch {
    // Storage full or disabled: recents are a convenience.
  }
}

/**
 * Client-side stand-in for a search category (§7.4, phase 5 removes it): cmdk's `command-score` over `text(item)`,
 * best first, zero-score rows dropped. An empty query returns `items` unchanged.
 */
export function localMatches<T>(items: readonly T[], query: string, text: (item: T) => string): T[] {
  const q = query.trim()
  if (!q) return [...items]
  return items
    .map((item) => ({ item, score: defaultFilter(text(item), q) }))
    .filter((x) => x.score > 0)
    .sort((a, b) => b.score - a.score)
    .map((x) => x.item)
}

/** What a key does in the picker (§7.7); `null` leaves it to cmdk (arrows) or the input. */
export type KeyAction =
  | { type: 'attach' }
  | { type: 'toggle' }
  | { type: 'tab'; dir: 1 | -1 }
  | { type: 'jump'; index: number }
  | { type: 'preview'; open: boolean }
  | { type: 'back' }
  | { type: 'close' }

export function keyAction(
  e: { key: string; metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean },
  queryEmpty: boolean,
): KeyAction | null {
  const mod = e.metaKey || e.ctrlKey
  if (e.key === 'Escape') return { type: 'close' }
  if (e.key === 'Enter') return mod ? { type: 'toggle' } : { type: 'attach' }
  if (e.key === 'Tab') return { type: 'tab', dir: e.shiftKey ? -1 : 1 }
  if (mod && /^[1-6]$/.test(e.key)) return { type: 'jump', index: Number(e.key) - 1 }
  if (e.key === 'ArrowRight' && queryEmpty) return { type: 'preview', open: true }
  if (e.key === 'ArrowLeft' && queryEmpty) return { type: 'preview', open: false }
  if (e.key === 'Backspace' && queryEmpty) return { type: 'back' }
  return null
}

/** The `@word` replaced by nothing once a row is picked (§7.8); a following space goes with it. */
export function stripAt(text: string, start: number, word: string): string {
  const end = start + 1 + word.length
  return text.slice(0, start) + text.slice(text[end] === ' ' ? end + 1 : end)
}
