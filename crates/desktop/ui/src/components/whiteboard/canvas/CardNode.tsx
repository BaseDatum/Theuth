// §8.5 card chrome: title bar (title + kind), stacked edge and the `v2 of 3` scrubber, dimmed + tag chip, placeholder
// shimmer, selected outline + four handles, entered border; a shield over every live body until the card is entered.
import { Handle, NodeResizer, NodeToolbar, Position, type Node, type NodeProps } from '@xyflow/react'
import { useNavigate } from '@tanstack/react-router'
import { ChevronLeft, ChevronRight, ExternalLink, EyeOff, Eye, Archive, PanelRight, Pin, PinOff, RefreshCw } from 'lucide-react'
import { memo, useRef, useState } from 'react'
import { openViewFromCard } from '@/components/tiles/open'
import { useStore as useZustand } from 'zustand'
import { CardBoundary, Chip, hostOf, KIND_NAME, LiveBody, Snapshot } from './cards/bodies'
import { useWebPreview, WebFavicon } from './cards/WebCard'
import { useCanvas } from './context'
import { versionOf, type Card } from './model'
import { archive, dim, patchCard, restore } from './ops'
import { IN_PLACE } from './ops'
import { CardInk } from './InkLayer'
import { toAbsolute, type CardNodeData } from './binding'
import { isBare } from './send'
import { useResizeEnd } from './resize'
import { viewCardPath } from '../view-card'

export type CardNodeType = Node<CardNodeData, 'card'>

const PORTS = [
  ['top', Position.Top],
  ['right', Position.Right],
  ['bottom', Position.Bottom],
  ['left', Position.Left],
] as const

function Scrubber({ card }: { card: Card }) {
  const { store } = useCanvas()
  const shown = useZustand(store, (s) => s.scrub[card.id]) ?? card.versions.length
  const n = card.versions.length
  const go = (v: number) => {
    const scrub = { ...store.getState().scrub }
    if (v >= n) delete scrub[card.id]
    else scrub[card.id] = v
    store.getState().set({ scrub })
  }
  return (
    <div className="wb-scrub nodrag nopan" role="group" aria-label="Versions">
      <button type="button" aria-label="Previous version" disabled={shown <= 1} onClick={() => go(shown - 1)}>
        <ChevronLeft className="size-3.5" />
      </button>
      <span>
        v{shown} of {n}
      </span>
      <button type="button" aria-label="Next version" disabled={shown >= n} onClick={() => go(shown + 1)}>
        <ChevronRight className="size-3.5" />
      </button>
    </div>
  )
}

/** A view card's route in a pane (on a phone: the route itself). Only view cards mount it: it needs the app router. */
function OpenInPane({ path, title }: { path: string; title: string }) {
  const navigate = useNavigate()
  return (
    <button type="button" onClick={() => openViewFromCard({ path, title: title || undefined }, { navigate: (href) => void navigate({ href }) })}>
      <PanelRight className="size-4" /> Open in pane
    </button>
  )
}

function Toolbar({ card, refreshPreview }: { card: Card; refreshPreview: (() => void) | null }) {
  const { doc, open, stopCapturing, readOnly } = useCanvas()
  const [tagging, setTagging] = useState(false)
  const [tag, setTag] = useState('')
  const source = card.kind === 'view' ? versionOf(card)?.source : undefined
  const viewPath = source?.t === 'view' ? viewCardPath(source.path) : null
  if (tagging) {
    return (
      <form
        className="wb-tools nodrag nopan"
        onSubmit={(e) => {
          e.preventDefault()
          dim(doc, [card.id], tag)
          stopCapturing()
          setTagging(false)
          setTag('')
        }}
      >
        <input autoFocus aria-label="Why is it dimmed?" placeholder="Why? (shown under the card)" value={tag} maxLength={120} onChange={(e) => setTag(e.target.value)} onKeyDown={(e) => e.stopPropagation()} />
        <button type="submit">Dim</button>
        <button type="button" onClick={() => setTagging(false)}>
          Cancel
        </button>
      </form>
    )
  }
  return (
    <div className="wb-tools nodrag nopan" role="toolbar" aria-label={`${card.title || 'Card'} actions`}>
      <button type="button" onClick={() => open(card.id)}>
        <ExternalLink className="size-4" /> Open
      </button>
      {viewPath && <OpenInPane path={viewPath} title={card.title} />}
      {refreshPreview && (
        <button type="button" aria-label="Refresh preview" onClick={refreshPreview}>
          <RefreshCw className="size-4" />
        </button>
      )}
      {!readOnly && (
        <>
          <span className="sep" />
          {card.state === 'dimmed' ? (
            <button type="button" aria-label="Undim" onClick={() => { restore(doc, [card.id]); stopCapturing() }}>
              <Eye className="size-4" />
            </button>
          ) : (
            <button type="button" aria-label="Dim with a reason" onClick={() => setTagging(true)}>
              <EyeOff className="size-4" />
            </button>
          )}
          {card.kind === 'page' && (
            <button
              type="button"
              aria-label={card.pin ? 'Unpin live' : 'Pin live (never demoted to its snapshot; demotion loses the page state)'}
              aria-pressed={card.pin === true}
              onClick={() => { patchCard(doc, card.id, { pin: card.pin ? undefined : true }); stopCapturing() }}
            >
              {card.pin ? <PinOff className="size-4" /> : <Pin className="size-4" />}
            </button>
          )}
          <button type="button" aria-label="Archive" onClick={() => { archive(doc, [card.id]); stopCapturing() }}>
            <Archive className="size-4" />
          </button>
        </>
      )}
    </div>
  )
}

export const CardNode = memo(function CardNode({ data, selected }: NodeProps<CardNodeType>) {
  const { card } = data
  const { store, lod, readOnly, enter, doc, stopCapturing, spot, board } = useCanvas()
  const entered = useZustand(store, (s) => s.entered === card.id)
  // Hand (and Present, which locks it) pans: no card chrome there, a click-selected card included.
  const hand = useZustand(store, (s) => s.mode === 'hand')
  const scrub = useZustand(store, (s) => s.scrub[card.id])
  const level = lod[card.id] ?? 'chip'
  const source = versionOf(card, scrub)?.source
  const web = source?.t === 'web' ? source.url : null
  const refreshPreview = useWebPreview(card)
  const lastTap = useRef(0)
  const versioned = !IN_PLACE.includes(card.kind)
  const stacked = versioned && card.versions.length > 1
  const placeholder = card.placeholder !== undefined
  // A diagram is just the diagram: no title bar; its title (if any) is a small caption on hover or selection.
  const bare = isBare(card)

  const cls = [
    'wb-card',
    card.kind === 'sticky' && 'sticky',
    bare && 'bare',
    stacked && 'stacked',
    card.state === 'dimmed' && 'dimmed',
    placeholder && 'placeholder',
    selected && !entered && 'selected',
    entered && 'entered',
    spot === card.id && 'spot',
  ].filter(Boolean).join(' ')
  const onResizeEnd = useResizeEnd((_, p) => {
    const at = toAbsolute(board, card.id, { x: p.x, y: p.y })
    patchCard(doc, card.id, { x: Math.round(at.x), y: Math.round(at.y), w: Math.round(p.width), h: Math.round(p.height) })
    stopCapturing()
  })

  return (
    <>
      {selected && !readOnly && !entered && !hand && (
        <NodeResizer
          minWidth={80}
          minHeight={60}
          color="var(--fg-accent)"
          lineStyle={{ borderColor: 'transparent' }}
          onResizeEnd={onResizeEnd}
        />
      )}
      {selected && !entered && !hand && (
        <NodeToolbar position={Position.Top} offset={14}>
          <Toolbar card={card} refreshPreview={refreshPreview} />
        </NodeToolbar>
      )}
      <div className={cls} data-card={card.id} aria-label={`${KIND_NAME[card.kind]}: ${card.title}`}>
        {bare ? (
          card.title && <div className="wb-caption">{card.title}</div>
        ) : (
          <div className="wb-head">
            {web !== null && <WebFavicon url={web} />}
            <span className="wb-title">{card.title || 'Untitled'}</span>
            <span className="wb-kind">
              {web !== null ? hostOf(web) : KIND_NAME[card.kind]}
              {versioned && card.versions.length > 0 ? ` · v${scrub ?? card.versions.length}` : ''}
            </span>
          </div>
        )}
        <div
          className="wb-clip"
          data-body
          onPointerUp={(e) => {
            // Double-tap (finger) or double-click enters the card; a pen never reaches here (the ink filter takes it).
            if (entered || placeholder || e.pointerType === 'pen' || (readOnly && card.kind === 'sticky')) return
            if (e.timeStamp - lastTap.current < 320) {
              lastTap.current = 0
              enter(card.id)
            } else lastTap.current = e.timeStamp
          }}
        >
          {placeholder ? (
            <div className="wb-shimmer">{card.placeholder || 'building…'}</div>
          ) : level === 'live' || entered ? (
            <CardBoundary card={card}>
              <LiveBody card={card} source={source} editing={entered} />
            </CardBoundary>
          ) : level === 'snapshot' && !scrub ? (
            <Snapshot card={card} />
          ) : (
            <Chip card={card} />
          )}
          {!entered && level === 'live' && !placeholder && <div className="wb-shield" />}
        </div>
        <CardInk card={card} />
        {stacked && <Scrubber card={card} />}
        {card.state === 'dimmed' && card.tag && <div className="wb-tag">{card.tag}</div>}
      </div>
      {/* ConnectionMode.Loose: every handle is a source and a target; ids are `s-<port>`. */}
      {PORTS.map(([name, pos]) => (
        <Handle key={name} id={`s-${name}`} type="source" position={pos} className="wb-handle" isConnectable={!readOnly} />
      ))}
    </>
  )
})
