// §2.3 links: explicit arrows with an optional label; tap → label, direction flip, style, delete.
import { BaseEdge, EdgeLabelRenderer, getBezierPath, type Edge, type EdgeProps } from '@xyflow/react'
import { ArrowLeftRight, Trash2 } from 'lucide-react'
import { memo, useState } from 'react'
import { useCanvas } from './context'
import type { Link } from './model'
import { patchLink, removeLink } from './ops'

export type LinkEdgeType = Edge<{ link: Link }, 'link'>

export const LinkEdge = memo(function LinkEdge(p: EdgeProps<LinkEdgeType>) {
  const link = p.data?.link
  const { doc, readOnly, stopCapturing } = useCanvas()
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
        style={{ strokeDasharray: link.style === 'dashed' ? '6 5' : undefined }}
      />
      {(link.label || (p.selected && !readOnly)) && (
        <EdgeLabelRenderer>
          <div className="nodrag nopan" style={{ position: 'absolute', transform: `translate(-50%, -50%) translate(${lx}px, ${ly}px)`, pointerEvents: 'all', zIndex: 5 }}>
            {p.selected && !readOnly ? (
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
