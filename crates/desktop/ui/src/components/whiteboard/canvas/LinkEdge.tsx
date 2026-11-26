// §2.3 links: explicit arrows with an optional label; tap near the line (a wide invisible hit stroke, canvas.css) →
// label, arrow, direction flip, style, delete.
import { BaseEdge, EdgeLabelRenderer, getBezierPath, type Edge, type EdgeProps } from '@xyflow/react'
import { ArrowLeftRight, Trash2 } from 'lucide-react'
import { memo, useState } from 'react'
import { useStore as useZustand } from 'zustand'
import { useCanvas } from './context'
import type { Link } from './model'
import { patchLink, removeLink } from './ops'

export type LinkEdgeType = Edge<{ link: Link }, 'link'>

export const LinkEdge = memo(function LinkEdge(p: EdgeProps<LinkEdgeType>) {
  const link = p.data?.link
  const { doc, readOnly, stopCapturing, store } = useCanvas()
  // Hand (and Present, which locks it) pans: no link toolbar there.
  const hand = useZustand(store, (s) => s.mode === 'hand' || s.present)
  // The outline panel's row under the pointer: the link shows as selected does.
  const hovered = useZustand(store, (s) => s.hover === p.id)
  const tools = !!p.selected && !readOnly && !hand
  const [label, setLabel] = useState<string | null>(null)
  const [path, lx, ly] = getBezierPath(p)
  if (!link) return null
  const nextArrow: Record<Link['arrow'], Link['arrow']> = { end: 'both', both: 'none', none: 'end' }
  const commit = (patch: Record<string, unknown>) => {
    patchLink(doc, link.id, patch)
    stopCapturing()
  }
  return (
    <>
      <BaseEdge
        id={p.id}
        path={path}
        markerEnd={p.markerEnd}
        markerStart={p.markerStart}
        interactionWidth={24}
        style={{ strokeDasharray: link.style === 'dashed' ? '6 5' : undefined, ...(hovered ? { stroke: 'var(--wb-accent)', strokeWidth: 2.2 } : {}) }}
      />
      {(link.label || tools) && (
        <EdgeLabelRenderer>
          {/* The toolbar keeps its screen size at any zoom (the label scales with the board). */}
          <div className="nodrag nopan" style={{ position: 'absolute', transform: `translate(-50%, -50%) translate(${lx}px, ${ly}px)${tools ? ' scale(calc(1 / var(--wb-zoom, 1)))' : ''}`, pointerEvents: 'all', zIndex: 5 }}>
            {tools ? (
              <div className="wb-tools" role="toolbar" aria-label="Link actions">
                <form
                  onSubmit={(e) => {
                    e.preventDefault()
                    if (label !== null) commit({ label: label.trim() ? label.slice(0, 80) : undefined })
                    setLabel(null)
                  }}
                >
                  <input aria-label="Link label" placeholder="label" value={label ?? link.label ?? ''} maxLength={80} onChange={(e) => setLabel(e.target.value)} onBlur={(e) => e.currentTarget.form?.requestSubmit()} onKeyDown={(e) => e.stopPropagation()} />
                </form>
                <button type="button" aria-label={`Arrow: ${link.arrow}`} onClick={() => commit({ arrow: nextArrow[link.arrow] })}>
                  <ArrowLeftRight className="size-4" />
                </button>
                <button type="button" aria-label="Flip direction" onClick={() => commit({ from: link.to, to: link.from, from_port: link.to_port, to_port: link.from_port })}>
                  ⇄
                </button>
                <button type="button" aria-pressed={link.style === 'dashed'} aria-label="Dashed" onClick={() => commit({ style: link.style === 'dashed' ? 'solid' : 'dashed' })}>
                  ┄
                </button>
                <button type="button" aria-label="Delete link" onClick={() => { removeLink(doc, link.id); stopCapturing() }}>
                  <Trash2 className="size-4" />
                </button>
              </div>
            ) : (
              <span className="wb-link-label">{link.label}</span>
            )}
          </div>
        </EdgeLabelRenderer>
      )}
    </>
  )
})
