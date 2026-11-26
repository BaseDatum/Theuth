// Screen-space overlays of the canvas: the lasso path, the lassoed-ink halo, the multi-selection bar, and the edge
// chevrons that point at the agent's off-screen placeholders (design §4.5).
import { useViewport } from '@xyflow/react'
import { Archive, ChevronRight, EyeOff, Pin, PinOff, Trash2, X } from 'lucide-react'
import { useState } from 'react'
import { pad, strokeWorldBox, union } from './geom'
import type { Board, Box, Id } from './model'

export function LassoPath({ pts }: { pts: [number, number][] }) {
  const { x, y, zoom } = useViewport()
  const d = pts.map(([px, py], i) => `${i ? 'L' : 'M'}${px * zoom + x},${py * zoom + y}`).join('') + 'Z'
  return (
    <svg className="pointer-events-none absolute inset-0 z-[7] size-full" aria-hidden>
      <path d={d} fill="color-mix(in oklab, var(--fg-accent) 8%, transparent)" stroke="var(--fg-accent)" strokeWidth={1.5} strokeDasharray="5 4" />
    </svg>
  )
}

/** A dashed box around the lassoed strokes (world space, in the viewport portal). */
export function InkSelection({ board, ids }: { board: Board; ids: Id[] }) {
  let u: Box | null = null
  for (const id of ids) {
    const s = board.ink[id]
    const w = s && strokeWorldBox(s, board.cards)
    if (w) u = union(u, w)
  }
  if (!u) return null
  const [bx, by, bw, bh] = pad(u, 6)
  return (
    <svg className="wb-ink" width={1} height={1} style={{ zIndex: 1_000_001 }} aria-hidden>
      <rect x={bx} y={by} width={bw} height={bh} rx={6} fill="none" stroke="var(--fg-accent)" strokeWidth={1.5} strokeDasharray="5 4" vectorEffect="non-scaling-stroke" />
    </svg>
  )
}

export function SelectionBar(p: {
  board: Board
  selection: Id[]
  ink: Id[]
  onDim(tag: string): void
  onPin(pinned: boolean): void
  onDelete(): void
  onClear(): void
}) {
  const [tagging, setTagging] = useState(false)
  const [tag, setTag] = useState('')
  const cards = p.selection.filter((id) => p.board.cards[id]).length
  const frames = p.selection.filter((id) => p.board.frames[id]).length
  const links = p.selection.filter((id) => p.board.links[id]).length
  const marks = p.selection.filter((id) => p.board.marks[id])
  const pinned = marks.length > 0 && marks.every((id) => p.board.marks[id]!.state === 'pinned')
  const parts = [
    cards && `${cards} card${cards === 1 ? '' : 's'}`,
    frames && `${frames} frame${frames === 1 ? '' : 's'}`,
    links && `${links} link${links === 1 ? '' : 's'}`,
    p.ink.length && `${p.ink.length} stroke${p.ink.length === 1 ? '' : 's'}`,
  ].filter(Boolean)
  // Cards are archived (restorable); frames (their cards stay), links and ink are removed.
  const rest = frames + links + p.ink.length > 0
  const deleteLabel = cards ? (rest ? 'Archive cards, delete the rest' : 'Archive cards') : 'Delete selection'
  return (
    <div className="wb-menu" style={{ left: '50%', top: 'calc(var(--safe-top, 0px) + 16px)', transform: 'translateX(-50%)' }}>
      {tagging ? (
        <form
          className="wb-tools"
          onSubmit={(e) => {
            e.preventDefault()
            p.onDim(tag)
            setTagging(false)
            setTag('')
          }}
        >
          <input autoFocus aria-label="Why are they dimmed?" placeholder="Why? (shown under each card)" value={tag} maxLength={120} onChange={(e) => setTag(e.target.value)} onKeyDown={(e) => e.stopPropagation()} />
          <button type="submit">Dim</button>
          <button type="button" onClick={() => setTagging(false)}>
            Cancel
          </button>
        </form>
      ) : (
        <div className="wb-tools" role="toolbar" aria-label="Selection">
          <span className="px-2 text-fg-secondary">{parts.join(' · ') || `${p.selection.length} selected`}</span>
          {cards > 0 && (
            <button type="button" aria-label="Dim with a reason" onClick={() => setTagging(true)}>
              <EyeOff className="size-4" />
            </button>
          )}
          {marks.length > 0 && (
            <button type="button" aria-label={pinned ? 'Unpin marks' : 'Pin marks'} onClick={() => p.onPin(!pinned)}>
              {pinned ? <PinOff className="size-4" /> : <Pin className="size-4" />}
            </button>
          )}
          <button type="button" aria-label={deleteLabel} title={deleteLabel} onClick={p.onDelete}>
            {cards ? <Archive className="size-4" /> : <Trash2 className="size-4" />}
          </button>
          <button type="button" aria-label="Clear selection" onClick={p.onClear}>
            <X className="size-4" />
          </button>
        </div>
      )}
    </div>
  )
}

/** Chevrons at the canvas edge pointing at placeholders the agent is filling off screen; tap → pan there. */
export function Offscreen({ board, width, height, onGo }: { board: Board; width: number; height: number; onGo(id: Id): void }) {
  const { x, y, zoom } = useViewport()
  if (width === 0 || height === 0) return null
  const out: { id: Id; left: number; top: number; angle: number; label: string }[] = []
  for (const c of Object.values(board.cards)) {
    if (c.placeholder === undefined || c.state === 'archived') continue
    const cx = (c.x + c.w / 2) * zoom + x
    const cy = (c.y + c.h / 2) * zoom + y
    const right = (c.x + c.w) * zoom + x
    const bottom = (c.y + c.h) * zoom + y
    const visible = right > 0 && c.x * zoom + x < width && bottom > 0 && c.y * zoom + y < height
    if (visible) continue
    const m = 28
    out.push({
      id: c.id,
      left: Math.min(width - m, Math.max(m, cx)),
      top: Math.min(height - m, Math.max(m, cy)),
      angle: (Math.atan2(cy - height / 2, cx - width / 2) * 180) / Math.PI,
      label: c.placeholder || c.title,
    })
  }
  return (
    <>
      {out.map((o) => (
        <button
          key={o.id}
          type="button"
          aria-label={`Go to ${o.label} (being made off screen)`}
          className="absolute z-[8] grid size-11 -translate-x-1/2 -translate-y-1/2 place-items-center rounded-full border border-border-subtle bg-layer-1/90 text-fg-accent shadow-md backdrop-blur"
          style={{ left: o.left, top: o.top }}
          onClick={() => onGo(o.id)}
        >
          <ChevronRight className="size-5" style={{ transform: `rotate(${o.angle}deg)` }} />
        </button>
      ))}
    </>
  )
}
