// §2.4 frames: a named region (dashed outline, title on the border), a React Flow parent node of its cards.
// Grabbed by its title or its border: a hit band a constant 12 px on screen (`--wb-hit`, canvas.css) straddles the
// outline; the interior passes through to the canvas and the cards inside.
import { NodeResizer, NodeToolbar, Position, type Node, type NodeProps } from '@xyflow/react'
import { Trash2 } from 'lucide-react'
import { memo, useState } from 'react'
import { useStore as useZustand } from 'zustand'
import { useCanvas } from './context'
import { frameAround, resizeMembership, type FrameNodeData } from './binding'
import { PRESETS, type FrameColour } from './model'
import { patchFrame, removeFrame, resizeFrame } from './ops'
import { useResizeEnd } from './resize'

export type FrameNodeType = Node<FrameNodeData, 'frame'>

const COLOURS: FrameColour[] = ['neutral', 'blue', 'green', 'amber', 'red', 'violet']
const SWATCH: Record<FrameColour, string> = {
  neutral: 'var(--fg-tertiary)', blue: 'var(--wb-blue)', green: 'var(--wb-green)', amber: 'var(--wb-amber)', red: 'var(--wb-red)', violet: 'var(--wb-violet)',
}
const SIDES = ['top', 'right', 'bottom', 'left'] as const

export const FrameNode = memo(function FrameNode({ data, selected }: NodeProps<FrameNodeType>) {
  const { frame } = data
  const { doc, readOnly, stopCapturing, store, board } = useCanvas()
  // Hand (and Present, which locks it) pans: a frame shows no chrome there and its band and title take no pointer.
  const hand = useZustand(store, (s) => s.mode === 'hand' || s.present)
  const [title, setTitle] = useState<string | null>(null)
  const preset = frame.preset ? ` · ${frame.preset} ${PRESETS[frame.preset][0]}×${PRESETS[frame.preset][1]}` : ''
  const onResizeEnd = useResizeEnd((_, p) => {
    const rect = { x: Math.round(p.x), y: Math.round(p.y), w: Math.round(p.width), h: Math.round(p.height) }
    resizeFrame(doc, frame.id, rect, resizeMembership(board, frame.id, [rect.x, rect.y, rect.w, rect.h]))
    stopCapturing()
  })
  const members = Object.values(board.cards).filter((c) => c.frame === frame.id && c.state !== 'archived')
  const fit = () => {
    const r = frameAround(members.map((c) => [c.x, c.y, c.w, c.h]))
    if (!r) return
    resizeFrame(doc, frame.id, { x: Math.round(r[0]), y: Math.round(r[1]), w: Math.round(r[2]), h: Math.round(r[3]) }, resizeMembership(board, frame.id, r))
    stopCapturing()
  }
  return (
    <>
      {selected && !readOnly && !hand && (
        <>
          <NodeResizer
            minWidth={160}
            minHeight={120}
            color="var(--fg-accent)"
            lineStyle={{ borderColor: 'transparent' }}
            onResizeEnd={onResizeEnd}
          />
          <NodeToolbar position={Position.Top} align="start" offset={18}>
            <div className="wb-tools nodrag nopan" role="toolbar" aria-label={`Frame ${frame.title} actions`}>
              {title === null ? (
                <button type="button" onClick={() => setTitle(frame.title)}>
                  Rename
                </button>
              ) : (
                <form
                  onSubmit={(e) => {
                    e.preventDefault()
                    patchFrame(doc, frame.id, { title: title.slice(0, 80) })
                    stopCapturing()
                    setTitle(null)
                  }}
                >
                  <input autoFocus aria-label="Frame title" value={title} maxLength={80} onChange={(e) => setTitle(e.target.value)} onBlur={() => setTitle(null)} onKeyDown={(e) => e.stopPropagation()} />
                </form>
              )}
              <button type="button" disabled={members.length === 0} onClick={fit} title="Fit the frame to its cards">
                Fit to cards
              </button>
              <span className="sep" />
              {COLOURS.map((c) => (
                <button key={c} type="button" aria-label={`Colour ${c}`} aria-pressed={frame.colour === c} onClick={() => { patchFrame(doc, frame.id, { colour: c }); stopCapturing() }}>
                  <span className="size-3.5 rounded-full" style={{ background: SWATCH[c] }} />
                </button>
              ))}
              <span className="sep" />
              <button type="button" aria-label="Delete frame (its cards stay)" onClick={() => { removeFrame(doc, frame.id); stopCapturing() }}>
                <Trash2 className="size-4" />
              </button>
            </div>
          </NodeToolbar>
        </>
      )}
      <div className={selected ? 'wb-frame selected' : 'wb-frame'} data-colour={frame.colour} aria-label={`Frame ${frame.title}`}>
        {SIDES.map((s) => <div key={s} className="wb-frame-band" data-side={s} aria-hidden />)}
        <span className="wb-frame-title">
          {frame.title || 'Frame'}
          {preset}
        </span>
      </div>
    </>
  )
})
