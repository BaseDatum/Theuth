// 218 §8.8 "+ Add": the cards the owner adds from the cluster or the outline — always at `handle.freeSpot` (near the
// view centre, on no card and no ink), sized like the engine's placement defaults (contract §3.1 "Placement").

import type * as Y from 'yjs'
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
}

export type Spot = (w: number, h: number) => { x: number; y: number }

export function place(doc: Y.Doc, spot: Spot, kind: CardKind, title: string, source: Source, size?: [number, number]): Id {
  const [w, h] = size ?? SIZE[kind]
  const { x, y } = spot(w, h)
  return addCard(doc, { kind, title, x, y, w, h, source })
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
