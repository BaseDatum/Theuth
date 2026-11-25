// §2.4 frames: a named region (dashed outline, title on the border), a React Flow parent node of its cards.
import { NodeResizer, NodeToolbar, Position, type Node, type NodeProps } from '@xyflow/react'
import { Trash2 } from 'lucide-react'
import { memo, useState } from 'react'
import { useCanvas } from './context'
import type { FrameNodeData } from './binding'
import { PRESETS, type FrameColour } from './model'
import { patchFrame, removeFrame } from './ops'

export type FrameNodeType = Node<FrameNodeData, 'frame'>

const COLOURS: FrameColour[] = ['neutral', 'blue', 'green', 'amber', 'red', 'violet']
const SWATCH: Record<FrameColour, string> = {
  neutral: 'var(--fg-tertiary)', blue: '#1f6feb', green: '#2f9e44', amber: '#b7791f', red: '#d9480f', violet: '#7048e8',
}

export const FrameNode = memo(function FrameNode({ data, selected }: NodeProps<FrameNodeType>) {
  const { frame } = data
  const { doc, readOnly, stopCapturing } = useCanvas()
  const [title, setTitle] = useState<string | null>(null)
  const preset = frame.preset ? ` · ${frame.preset} ${PRESETS[frame.preset][0]}×${PRESETS[frame.preset][1]}` : ''
  return (
    <>
      {selected && !readOnly && (
        <>
          <NodeResizer
            minWidth={160}
            minHeight={120}
            color="var(--accent)"
            lineStyle={{ borderColor: 'transparent' }}
            onResizeEnd={(_, p) => {
              patchFrame(doc, frame.id, { x: Math.round(p.x), y: Math.round(p.y), w: Math.round(p.width), h: Math.round(p.height) })
              stopCapturing()
            }}
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
        <span className="wb-frame-title">
          {frame.title || 'Frame'}
          {preset}
        </span>
      </div>
    </>
  )
})
