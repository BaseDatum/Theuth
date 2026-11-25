// §8.6: all committed ink, rendered by perfect-freehand into SVG — free strokes in a world-space layer above the
// cards (inside React Flow's viewport transform), anchored strokes inside their card, scaled to its current size.
import { memo } from 'react'
import { useStore as useZustand } from 'zustand'
import { inkCss } from '../ink/colour'
import { lookOf } from '../ink/life'
import { MARKER_OPACITY, strokePath } from '../ink/render'
import { scaleOf } from './geom'
import { useCanvas } from './context'
import type { Board, Card, Id, Stroke } from './model'

const StrokePath = memo(function StrokePath({ stroke, dark, opacity, pinned }: { stroke: Stroke; dark: boolean; opacity: number; pinned: boolean }) {
  const o = opacity * (stroke.tool === 'marker' ? MARKER_OPACITY : 1)
  return <path d={strokePath(stroke)} fill={inkCss(stroke.colour, dark)} opacity={o} className={pinned ? 'pinned' : undefined} data-stroke={stroke.id} />
})

function useView() {
  const { store } = useCanvas()
  const showAnswered = useZustand(store, (s) => s.showAnswered)
  const scrub = useZustand(store, (s) => s.scrub)
  return { showAnswered, scrub }
}

function strokesOf(board: Board, card: Id | null): Stroke[] {
  const out: Stroke[] = []
  for (const s of Object.values(board.ink)) if ((s.card ?? null) === card) out.push(s)
  return out.sort((a, b) => a.at - b.at || (a.id < b.id ? -1 : 1))
}

function Paths({ strokes, board }: { strokes: Stroke[]; board: Board }) {
  const { dark } = useCanvas()
  const view = useView()
  return (
    <>
      {strokes.map((s) => {
        const look = lookOf(board, s, view)
        return look.opacity > 0 ? <StrokePath key={s.id} stroke={s} dark={dark} opacity={look.opacity} pinned={look.pinned} /> : null
      })}
    </>
  )
}

/** Free strokes, world coordinates; mounted in a ViewportPortal. */
export function FreeInk() {
  const { board } = useCanvas()
  const strokes = strokesOf(board, null)
  if (strokes.length === 0) return null
  return (
    <svg className="wb-ink" width={1} height={1} style={{ zIndex: 1_000_000 }} aria-hidden>
      <Paths strokes={strokes} board={board} />
    </svg>
  )
}

/** A card's anchored strokes, card-local at capture size, scaled to the card's current size. */
export function CardInk({ card }: { card: Card }) {
  const { board } = useCanvas()
  const strokes = strokesOf(board, card.id)
  if (strokes.length === 0) return null
  return (
    <svg className="wb-ink" width={card.w} height={card.h} style={{ zIndex: 4 }} aria-hidden>
      {strokes.map((s) => {
        const [sx, sy] = scaleOf(s, card)
        return (
          <g key={s.id} transform={sx === 1 && sy === 1 ? undefined : `scale(${sx} ${sy})`}>
            <Paths strokes={[s]} board={board} />
          </g>
        )
      })}
    </svg>
  )
}
