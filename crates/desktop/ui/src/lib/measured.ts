// Design 135 §2: nothing that has been measured once should have to be measured again to find its size. Row heights
// (by row key) and async-surface heights (by `<entry>/<hash>`) are remembered per session at one content-width bucket,
// kept across remounts in a module LRU and across app runs in localStorage. A different bucket starts empty — a
// stale height is worse than an estimate. `estimateRow` is the content-based guess for a row never measured.
import { createContext } from 'react'
import type { Row } from '@/components/chat/rows'
import { LruCache } from './diagram/elk'

/** Heights measured in one session at one content width: rows by entry id, surfaces by `<entry>/<hash>`. */
export type Heights = { width: number; h: Map<string, number> }

const SESSIONS = 32
/** Most-recent keys kept in localStorage per session. */
export const PERSIST_KEYS = 4000
const PERSIST_DELAY = 1000
const storageKey = (session: string) => `theuth_heights:${session}`

const sessions = new LruCache<Heights>(SESSIONS)
/** Session → earliest time its debounced `persist` may run; absent = no timer armed. `persist` clears it. */
const due = new Map<string, number>()

/** One timer per session, re-armed for the remainder while `remember` keeps pushing the deadline. */
function arm(session: string, ms: number): void {
  setTimeout(() => {
    const d = due.get(session)
    if (d === undefined) return
    const left = d - Date.now()
    if (left > 0) arm(session, left)
    else persist(session)
  }, ms)
}

export const bucket = (contentWidth: number): number => Math.round(contentWidth / 16) * 16

function load(session: string, width: number): Heights {
  try {
    const raw = globalThis.localStorage?.getItem(storageKey(session))
    if (raw) {
      const { w, h } = JSON.parse(raw) as { w: number; h: Record<string, number> }
      if (w === width) return { width, h: new Map(Object.entries(h)) }
    }
  } catch {
    // corrupt or unavailable storage: start empty
  }
  return { width, h: new Map() }
}

/** The session's heights at `width`'s bucket; a stored bucket ≠ width starts empty (never restore). */
export function heights(session: string, width: number): Heights {
  const w = bucket(width)
  let hs = sessions.get(session)
  if (!hs) {
    hs = load(session, w)
    sessions.set(session, hs)
  }
  if (hs.width !== w) {
    hs = { width: w, h: new Map() }
    sessions.set(session, hs)
  }
  return hs
}

export function remember(session: string, width: number, key: string, px: number): void {
  const { h } = heights(session, width)
  // delete + set: Map order is recency, so `persist` keeps the most recent keys.
  h.delete(key)
  h.set(key, px)
  if (!due.has(session)) arm(session, PERSIST_DELAY)
  due.set(session, Date.now() + PERSIST_DELAY)
}

export function recall(session: string, width: number, key: string): number | undefined {
  return heights(session, width).h.get(key)
}

/** localStorage `theuth_heights:<session>` = `{ w, h }`, the ≤ 4000 most recent keys; quota errors are swallowed. */
export function persist(session: string): void {
  due.delete(session)
  const hs = sessions.get(session)
  if (!hs) return
  const entries = [...hs.h]
  const h: Record<string, number> = {}
  for (const [k, px] of entries.slice(Math.max(0, entries.length - PERSIST_KEYS))) h[k] = px
  try {
    globalThis.localStorage?.setItem(storageKey(session), JSON.stringify({ w: hs.width, h }))
  } catch {
    // quota or unavailable storage
  }
}

/** `{ session, width }` of the transcript a `Reserve` sits in; `null` outside one (artifact viewers, boards). */
export const MeasuredContext = createContext<{ session: string; width: number } | null>(null)

// ---- estimateRow -------------------------------------------------------------------------------------------------

/** Body text: 15 px font ≈ 7.2 px per character; paragraphs are `leading-[26px]`. */
const CHAR = 7.2
const LINE = 26
const FLOOR = 96
const FENCE: Record<string, number> = { mermaid: 320, 'json-render': 320, openui: 240 }

const charsPerLine = (contentWidth: number) => Math.max(20, Math.floor(contentWidth / CHAR))

/** Markdown at `contentWidth`: paragraphs by wrapped lines, fences by language, tables by rows, images flat. */
export function estimateText(text: string, contentWidth: number): number {
  const cpl = charsPerLine(contentWidth)
  let px = 0
  let fence: { lang: string; lines: number } | null = null
  let table = 0
  let paragraph = 0
  const flushParagraph = () => {
    if (paragraph > 0) px += Math.ceil(paragraph / cpl) * LINE + 8
    paragraph = 0
  }
  const flushTable = () => {
    if (table > 0) px += table * 32 + 40
    table = 0
  }
  for (const line of text.split('\n')) {
    if (fence) {
      if (line.startsWith('```')) {
        px += FENCE[fence.lang] ?? fence.lines * 20 + 40
        fence = null
      } else fence.lines++
      continue
    }
    if (line.startsWith('```')) {
      flushParagraph()
      flushTable()
      fence = { lang: line.slice(3).trim().split(/\s/)[0] ?? '', lines: 0 }
      continue
    }
    if (line.startsWith('|')) {
      flushParagraph()
      table++
      continue
    }
    flushTable()
    const t = line.trim()
    if (t === '') flushParagraph()
    else if (t.startsWith('![')) {
      flushParagraph()
      px += 240
    } else paragraph += t.length + 1
  }
  if (fence) px += FENCE[fence.lang] ?? fence.lines * 20 + 40
  flushParagraph()
  flushTable()
  return px
}

/** Content-based height guess for a row never measured at `contentWidth`: the point is a small first-measure delta. */
export function estimateRow(row: Row, contentWidth: number): number {
  let px: number
  if ('gap' in row) px = 36
  else if ('prompt' in row) px = 160
  else if ('run' in row) px = 56
  else {
    const e = row.entry
    px = 'text' in e && typeof e.text === 'string' ? 16 + estimateText(e.text, contentWidth) : 56
    if (e.kind === 'assistant' && !e.open) px += 44 // the action bar
    else if (e.kind === 'tool') px = 56
  }
  return Math.max(FLOOR, Math.round(px))
}
