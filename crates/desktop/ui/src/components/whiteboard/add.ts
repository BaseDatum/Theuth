// 218 §8.8 "+ Add": the cards the owner adds from the cluster or the outline — always at `handle.freeSpot` (near the
// view centre, on no card and no ink), sized like the engine's placement defaults (contract §3.1 "Placement").

import type * as Y from 'yjs'
import { fitsContent } from './canvas/fit'
import { addCard } from './canvas/ops'
import { ulid } from './canvas/ids'
import type { CardKind, Id, Source } from './canvas/model'

export const SIZE: Record<CardKind, [number, number]> = {
  page: [360, 270],
  sticky: [220, 150],
  diagram: [360, 260],
  excerpt: [420, 300],
  diff: [420, 300],
  terminal: [420, 300],
  image: [320, 240],
  web: [320, 240],
  note: [320, 240],
  session: [320, 240],
  view: [560, 400],
  task: [280, 168],
  deck: [560, 420],
}

export type Spot = (w: number, h: number) => { x: number; y: number }

/** `size` given: placed at it, never sized to content; else the default by kind, sized to content once it renders. */
export function place(doc: Y.Doc, spot: Spot, kind: CardKind, title: string, source: Source, size?: [number, number]): Id {
  const [w, h] = size ?? SIZE[kind]
  const { x, y } = spot(w, h)
  return addCard(doc, { kind, title, x, y, w, h, source, fit: !size && fitsContent({ kind }), sized: !!size })
}

/** A URL the web card accepts: http(s) only; a bare host gets `https://`. */
export function asUrl(text: string): string | null {
  const t = text.trim()
  if (!t || /\s/.test(t)) return null
  try {
    const u = new URL(/^[a-z][a-z0-9+.-]*:/i.test(t) ? t : `https://${t}`)
    return (u.protocol === 'http:' || u.protocol === 'https:') && u.hostname.includes('.') ? u.href : null
  } catch {
    return null
  }
}

/**
 * 218 review B1: the URL a stored `web` card may show, or null. The value is agent-writable, so only `http:`/`https:`
 * with a dotted host passes — never `asset:`, `tauri:`, `theuth-page:`, `file:`, `javascript:`, `data:` or a bare
 * `localhost`/dev URL that would load app content inside the main webview. No scheme is added: the stored text must
 * already be an absolute http(s) URL.
 */
export function webUrl(raw: unknown): string | null {
  if (typeof raw !== 'string' || !/^https?:\/\//i.test(raw.trim())) return null
  try {
    const u = new URL(raw.trim())
    if (u.protocol !== 'http:' && u.protocol !== 'https:') return null
    if (!u.hostname.includes('.') || u.username || u.password) return null
    return u.href
  } catch {
    return null
  }
}

/** An image's card size: its aspect, the default card's area. */
export function imageSize(w: number, h: number): [number, number] {
  if (!(w > 0 && h > 0)) return SIZE.image
  const long = 360
  return w >= h ? [long, Math.max(80, Math.round((long * h) / w))] : [Math.max(80, Math.round((long * w) / h)), long]
}

function naturalSize(blob: Blob): Promise<[number, number]> {
  const { promise, resolve } = Promise.withResolvers<[number, number]>()
  const url = URL.createObjectURL(blob)
  const img = new Image()
  const done = (size: [number, number]) => {
    resolve(size)
    URL.revokeObjectURL(url)
  }
  img.onload = () => done([img.naturalWidth, img.naturalHeight])
  img.onerror = () => done([0, 0])
  img.src = url
  return promise
}

/** Store the image as `img/<ulid>` and place an image card for it. */
export async function placeImage(doc: Y.Doc, spot: Spot, blobPut: (key: string, blob: Blob) => Promise<void>, blob: Blob, title: string): Promise<Id> {
  const key = `img/${ulid()}`
  await blobPut(key, blob)
  const [w, h] = await naturalSize(blob)
  return place(doc, spot, 'image', title, { t: 'image', blob: key }, imageSize(w, h))
}

/** From the clipboard: an image → image card; a link → web card; text → sticky (≤ 280 chars). */
export async function placeFromClipboard(doc: Y.Doc, spot: Spot, blobPut: (key: string, blob: Blob) => Promise<void>): Promise<Id | null> {
  const clip = navigator.clipboard
  if (clip?.read) {
    try {
      for (const item of await clip.read()) {
        const type = item.types.find((t) => t.startsWith('image/'))
        if (type) return placeImage(doc, spot, blobPut, await item.getType(type), 'Pasted image')
      }
    } catch {
      // Permission or no image: fall through to text.
    }
  }
  const text = (await clip?.readText?.().catch(() => '')) ?? ''
  if (!text.trim()) return null
  const url = asUrl(text)
  if (url) return place(doc, spot, 'web', new URL(url).hostname, { t: 'web', url })
  return place(doc, spot, 'sticky', 'Sticky', { t: 'sticky', text: text.trim().slice(0, 280) })
}

/** `excerpt.text` limit (contract §2.2) and the lines a whole-file excerpt takes. */
export const EXCERPT_MAX = 16_000
export const EXCERPT_LINES = 80

/** `src/app.ts`, `src/app.ts:10`, `src/app.ts:10-40`, `src/app.ts#L10-L40` → path and an optional 1-based line range. */
export function parseFileRef(s: string): { path: string; start?: number; end?: number } | null {
  const t = s.trim()
  if (!t) return null
  const m = /^(.*?)(?:#L(\d+)(?:-L?(\d+))?|:(\d+)(?:-(\d+))?)$/.exec(t)
  if (!m || !m[1]) return { path: t }
  const a = Number(m[2] ?? m[4])
  const b = m[3] ?? m[5]
  return { path: m[1], start: a, end: b ? Math.max(a, Number(b)) : a }
}

/** The excerpt card's source for `text` (the whole file) and a range; no range → the first `EXCERPT_LINES` lines. */
export function excerptOf(path: string, text: string, lang: string | undefined, start?: number, end?: number): Extract<Source, { t: 'excerpt' }> {
  const lines = text.split('\n')
  const a = Math.min(Math.max(1, start ?? 1), Math.max(1, lines.length))
  const b = Math.min(lines.length, end ?? a + EXCERPT_LINES - 1)
  let body = lines.slice(a - 1, b).join('\n')
  let last = b
  if (body.length > EXCERPT_MAX) {
    body = body.slice(0, EXCERPT_MAX)
    last = a + body.split('\n').length - 1
  }
  return { t: 'excerpt', path, start: a, end: last, text: body, ...(lang ? { lang } : {}) }
}
