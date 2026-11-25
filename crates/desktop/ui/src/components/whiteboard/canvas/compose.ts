// `composeImage` (§8.2): the fallback pixels when there is no native capture — each card's stored snapshot (or a
// labelled box when none exists) with the vector ink drawn over it, long side ≤ maxSide.
import { inkCss } from '../ink/colour'
import { lookOf } from '../ink/life'
import { MARKER_OPACITY, strokePath } from '../ink/render'
import type { PlannedImage } from './api'
import { intersects, scaleOf, shownCards } from './geom'
import type { Board, Card, Id } from './model'
import { headerOf } from './send'

type Theme = { bg: string; card: string; border: string; fg: string; fg2: string }
const THEMES: Record<'light' | 'dark', Theme> = {
  light: { bg: '#e8dcc8', card: '#f3eadf', border: '#d6cbb8', fg: '#1a1612', fg2: '#6b6253' },
  dark: { bg: '#12100e', card: '#1c1915', border: '#34302a', fg: '#e8dcc8', fg2: '#a39a88' },
}

function loadImage(url: string): Promise<HTMLImageElement | null> {
  const { promise, resolve } = Promise.withResolvers<HTMLImageElement | null>()
  const img = new Image()
  img.onload = () => resolve(img)
  img.onerror = () => resolve(null)
  img.src = url
  return promise
}

function roundRect(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number) {
  ctx.beginPath()
  ctx.roundRect(x, y, w, h, r)
}

export async function composeImage(
  board: Board,
  image: PlannedImage,
  maxSide: number,
  o: { dark: boolean; blobUrl(key: string): Promise<string | null>; showAnswered: boolean; scrub: Record<Id, number> },
): Promise<Blob> {
  const [wx, wy, ww, wh] = image.world
  const s = Math.min(maxSide / Math.max(ww, wh), 2)
  const canvas = document.createElement('canvas')
  canvas.width = Math.max(1, Math.round(ww * s))
  canvas.height = Math.max(1, Math.round(wh * s))
  const ctx = canvas.getContext('2d')!
  const t = THEMES[o.dark ? 'dark' : 'light']
  ctx.fillStyle = t.bg
  ctx.fillRect(0, 0, canvas.width, canvas.height)
  ctx.setTransform(s, 0, 0, s, -wx * s, -wy * s)

  const cards = shownCards(board).filter((c) => intersects([c.x, c.y, c.w, c.h], image.world)).sort((a, b) => a.z - b.z)
  for (const c of cards) await drawCard(ctx, c, t, o.blobUrl)

  // Ink over the cards: free strokes in world space, anchored ones in their card's (scaled) space.
  for (const st of Object.values(board.ink).sort((a, b) => a.at - b.at)) {
    const look = lookOf(board, st, { showAnswered: o.showAnswered, scrub: o.scrub })
    if (look.opacity === 0) continue
    ctx.save()
    if (st.card) {
      const c = board.cards[st.card]
      if (!c || !intersects([c.x, c.y, c.w, c.h], image.world)) {
        ctx.restore()
        continue
      }
      const [sx, sy] = scaleOf(st, c)
      ctx.translate(c.x, c.y)
      ctx.scale(sx, sy)
    } else if (!intersects(st.bb, image.world)) {
      ctx.restore()
      continue
    }
    ctx.globalAlpha = look.opacity * (st.tool === 'marker' ? MARKER_OPACITY : 1)
    ctx.fillStyle = inkCss(st.colour, o.dark)
    ctx.fill(new Path2D(strokePath(st)))
    ctx.restore()
  }
  const { promise, resolve, reject } = Promise.withResolvers<Blob>()
  canvas.toBlob((b) => (b ? resolve(b) : reject(new Error('composeImage: empty canvas'))), 'image/png')
  return promise
}

async function drawCard(ctx: CanvasRenderingContext2D, c: Card, t: Theme, blobUrl: (key: string) => Promise<string | null>) {
  ctx.save()
  roundRect(ctx, c.x, c.y, c.w, c.h, 12)
  ctx.fillStyle = t.card
  ctx.fill()
  ctx.strokeStyle = t.border
  ctx.lineWidth = 1
  ctx.stroke()
  ctx.clip()
  const head = headerOf(c)
  if (head > 0) {
    ctx.fillStyle = t.fg
    ctx.font = '600 11.5px system-ui, sans-serif'
    ctx.textBaseline = 'middle'
    ctx.fillText(c.title || c.kind, c.x + 10, c.y + head / 2, c.w - 20)
    ctx.beginPath()
    ctx.moveTo(c.x, c.y + head)
    ctx.lineTo(c.x + c.w, c.y + head)
    ctx.stroke()
  }
  const url = c.snap ? await blobUrl(`snap/${c.id}/${c.snap.v}`) : null
  const img = url ? await loadImage(url) : null
  if (img) ctx.drawImage(img, c.x, c.y + head, c.w, c.h - head)
  else {
    // The labelled box: what the relay web client shows for a card nobody has snapshotted (design §3.4).
    ctx.fillStyle = t.fg2
    ctx.font = '500 12px system-ui, sans-serif'
    ctx.textAlign = 'center'
    const src = c.versions.at(-1)?.source
    const text = src?.t === 'sticky' ? src.text : `${c.kind} · no snapshot`
    const lines = text.split('\n').slice(0, 8)
    lines.forEach((l, i) => ctx.fillText(l, c.x + c.w / 2, c.y + head + 24 + i * 16, c.w - 16))
  }
  ctx.restore()
}
