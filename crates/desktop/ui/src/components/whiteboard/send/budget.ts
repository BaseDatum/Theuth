// 218 §8.8 / design §6.4: what a send may carry. One image per inked card (plus zoom images of small marks and one
// per free mark) — never a screenshot — inside a pixel budget: long side ≤ 768 px, at most 6 images and 1.2 MP in
// total. Over budget, in this order: scale everything down together (to a legibility floor), then drop the zoom
// images, then merge the images of one frame into one, then keep the first six. Pure; the sizes it returns are what
// the capture (or the canvas's composition) is asked for.

import type { Box, Id } from '../canvas/model'

export const LONG_SIDE = 768
export const MAX_IMAGES = 6
export const MAX_PIXELS = 1_200_000
/** Scaling together never shrinks an image's long side below this (marks stop being legible); merging takes over. */
export const MIN_LONG = 384

export type BudgetImage = {
  card: Id | null
  marks: Id[]
  /** World box of the image. */
  world: Box
  /** Window rect when fully on screen (native capture), else null (composition). */
  rect: DOMRect | null
  /** A second, zoomed image of this mark. */
  zoomOf?: Id
  /** The frame the image's card sits in; images of one frame may be merged. */
  frame?: Id
  /** Natural pixel size (screen rect × device pixel ratio, or the world box at zoom 1). */
  px: [number, number]
}
export type Sized = BudgetImage & { w: number; h: number }
export type Budgeted = { images: Sized[]; omitted: number }

const capLong = (w: number, h: number, long: number): [number, number] => {
  const s = Math.min(1, long / Math.max(w, h, 1))
  return [Math.max(1, Math.round(w * s)), Math.max(1, Math.round(h * s))]
}

const pixels = (xs: readonly Sized[]) => xs.reduce((n, i) => n + i.w * i.h, 0)
const fits = (xs: readonly Sized[]) => xs.length <= MAX_IMAGES && pixels(xs) <= MAX_PIXELS

const sized = (i: BudgetImage): Sized => {
  const [w, h] = capLong(i.px[0], i.px[1], LONG_SIDE)
  return { ...i, w, h }
}

/** Scale all together by one factor so the total meets `MAX_PIXELS`; with `floor`, no long side goes below `MIN_LONG`. */
function scaleTogether(xs: Sized[], floor: boolean): Sized[] {
  const total = pixels(xs)
  if (total <= MAX_PIXELS) return xs
  const s = Math.sqrt(MAX_PIXELS / total)
  return xs.map((i) => {
    const long = Math.max(i.w, i.h)
    const target = floor ? Math.max(Math.min(long, MIN_LONG), long * s) : long * s
    const [w, h] = capLong(i.w, i.h, Math.floor(target))
    return { ...i, w, h }
  })
}

const union = (a: Box, b: Box): Box => {
  const x = Math.min(a[0], b[0])
  const y = Math.min(a[1], b[1])
  return [x, y, Math.max(a[0] + a[2], b[0] + b[2]) - x, Math.max(a[1] + a[3], b[1] + b[3]) - y]
}

const unionRect = (a: DOMRect | null, b: DOMRect | null): DOMRect | null => {
  if (!a || !b) return null
  const x = Math.min(a.x, b.x)
  const y = Math.min(a.y, b.y)
  const w = Math.max(a.x + a.width, b.x + b.width) - x
  const h = Math.max(a.y + a.height, b.y + b.height) - y
  return typeof DOMRect === 'function' ? new DOMRect(x, y, w, h) : ({ x, y, width: w, height: h, left: x, top: y, right: x + w, bottom: y + h } as DOMRect)
}

/** Images of one frame become one image of their union (a "canvas" image: `card: null`); order kept by first member. */
export function mergeFrames(xs: readonly BudgetImage[]): BudgetImage[] {
  const out: BudgetImage[] = []
  const at = new Map<Id, number>()
  for (const i of xs) {
    const k = i.frame
    const j = k === undefined ? undefined : at.get(k)
    if (k === undefined || j === undefined) {
      if (k !== undefined) at.set(k, out.length)
      out.push(i)
      continue
    }
    const m = out[j] as BudgetImage
    const world = union(m.world, i.world)
    // Pixels per world unit of the first member carry over to the merged box.
    const ppw = m.px[0] / Math.max(1, m.world[2])
    out[j] = {
      card: null,
      marks: [...m.marks, ...i.marks.filter((x) => !m.marks.includes(x))],
      world,
      rect: unionRect(m.rect, i.rect),
      frame: k,
      px: [world[2] * ppw, world[3] * ppw],
    }
  }
  return out
}

export function fitBudget(input: readonly BudgetImage[]): Budgeted {
  let xs = input.map(sized)
  if (fits(xs)) return { images: xs, omitted: 0 }
  // 1. Scale together, keeping marks legible.
  xs = scaleTogether(xs, true)
  if (fits(xs)) return { images: xs, omitted: 0 }
  // 2. Drop the zoom images (their marks are on their card's image too).
  const unzoomed = input.filter((i) => !i.zoomOf)
  const dropped = input.length - unzoomed.length
  xs = scaleTogether(unzoomed.map(sized), true)
  if (fits(xs)) return { images: xs, omitted: dropped }
  // 3. Merge per frame.
  xs = scaleTogether(mergeFrames(unzoomed).map(sized), true)
  if (fits(xs)) return { images: xs, omitted: dropped }
  // 4. The first six, scaled without a floor.
  const kept = xs.slice(0, MAX_IMAGES)
  return { images: scaleTogether(kept, false), omitted: dropped + (xs.length - kept.length) }
}
