import { Link } from '@tanstack/react-router'
import { Maximize2, Minus, Plus } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { THEUTH } from '@/components/tasks/helpers'
import { cn } from '@/lib/utils'
import type { AgentSummary } from '@/rpc/bindings/AgentSummary'
import { ActiveSwitch, ActivityChip, AgentAvatar, BudgetBar, statusDot } from './identity'
import { CARD, type Node, layout } from './tree'

const PAD = 32

/**
 * Paperclip's OrgChart: cards on absolute coordinates from `layout`, elbow edges in an SVG layer underneath, one
 * `translate/scale` transform for pan (drag) and zoom (wheel, buttons). Fits the whole tree on first paint.
 */
export function OrgChart({ root }: { root: Node }) {
  const box = useRef<HTMLDivElement>(null)
  const l = layout(root)
  const [view, setView] = useState({ x: PAD, y: PAD, k: 1 })
  const drag = useRef<{ x: number; y: number; vx: number; vy: number } | null>(null)

  const fit = () => {
    const el = box.current
    if (!el) return
    const k = Math.min(1, (el.clientWidth - 2 * PAD) / l.width, (el.clientHeight - 2 * PAD) / l.height)
    setView({ k: Math.max(0.2, k), x: (el.clientWidth - l.width * k) / 2, y: PAD })
  }
  // Fit once per tree shape; the user's pan/zoom survives data refreshes.
  const shape = l.cards.map((c) => c.node.name).join(',')
  useEffect(fit, [shape])

  const zoom = (f: number, cx?: number, cy?: number) =>
    setView((v) => {
      const k = Math.min(2, Math.max(0.2, v.k * f))
      const el = box.current
      const px = cx ?? (el?.clientWidth ?? 0) / 2
      const py = cy ?? (el?.clientHeight ?? 0) / 2
      return { k, x: px - ((px - v.x) * k) / v.k, y: py - ((py - v.y) * k) / v.k }
    })

  return (
    <div
      ref={box}
      className={cn('relative min-h-0 flex-1 overflow-hidden bg-layer-1/40 select-none', drag.current ? 'cursor-grabbing' : 'cursor-grab')}
      onWheel={(e) => {
        const r = e.currentTarget.getBoundingClientRect()
        if (e.ctrlKey || e.metaKey) zoom(e.deltaY < 0 ? 1.1 : 0.9, e.clientX - r.left, e.clientY - r.top)
        else setView((v) => ({ ...v, x: v.x - e.deltaX, y: v.y - e.deltaY }))
      }}
      onPointerDown={(e) => {
        if (e.button !== 0) return
        drag.current = { x: e.clientX, y: e.clientY, vx: view.x, vy: view.y }
        e.currentTarget.setPointerCapture(e.pointerId)
      }}
      onPointerMove={(e) => {
        const d = drag.current
        if (d) setView((v) => ({ ...v, x: d.vx + e.clientX - d.x, y: d.vy + e.clientY - d.y }))
      }}
      onPointerUp={() => {
        drag.current = null
      }}
    >
      <svg className="pointer-events-none absolute inset-0 size-full" aria-hidden>
        <g transform={`translate(${view.x} ${view.y}) scale(${view.k})`}>
          {l.edges.map(({ from, to }) => {
            const x1 = from.x + CARD.w / 2
            const y1 = from.y + CARD.h
            const x2 = to.x + CARD.w / 2
            const my = y1 + CARD.gy / 2
            return <path key={to.node.name} d={`M${x1} ${y1} L${x1} ${my} L${x2} ${my} L${x2} ${to.y}`} fill="none" className="stroke-border" strokeWidth={1.5} />
          })}
        </g>
      </svg>
      <div className="absolute top-0 left-0" style={{ transform: `translate(${view.x}px, ${view.y}px) scale(${view.k})`, transformOrigin: '0 0' }}>
        {l.cards.map((p) => (
          <Card key={p.node.name} name={p.node.name} agent={p.node.agent} style={{ left: p.x, top: p.y, width: CARD.w, minHeight: CARD.h }} />
        ))}
      </div>
      <div className="absolute top-2 right-2 flex flex-col gap-1 rounded-lg border border-border-subtle bg-background p-1">
        <Button variant="ghost" size="icon-xs" aria-label="Zoom in" onClick={() => zoom(1.25)}>
          <Plus />
        </Button>
        <Button variant="ghost" size="icon-xs" aria-label="Zoom out" onClick={() => zoom(0.8)}>
          <Minus />
        </Button>
        <Button variant="ghost" size="icon-xs" aria-label="Fit to screen" onClick={fit}>
          <Maximize2 />
        </Button>
      </div>
    </div>
  )
}

function Card({ name, agent: a, style }: { name: string; agent: AgentSummary | null; style: React.CSSProperties }) {
  const title = a?.file.title ?? name
  return (
    <Link
      to="/agents/$name/$tab"
      params={{ name, tab: 'overview' }}
      draggable={false}
      onPointerDown={(e) => e.stopPropagation()}
      className={cn('absolute flex flex-col gap-1.5 rounded-lg border border-border-subtle bg-background p-2.5 text-sm shadow-xs hover:bg-layer-2', a?.file.paused && 'opacity-60')}
      style={style}
    >
      <div className="flex items-center gap-2">
        <span className="relative shrink-0">
          <AgentAvatar name={name} title={title} size={32} />
          <span className={cn('absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full border-2 border-background', a ? statusDot(a.status) : 'bg-success')} aria-hidden />
        </span>
        <span className="min-w-0 flex-1">
          <span className="block truncate font-semibold leading-tight">{title}</span>
          <span className="block truncate font-mono text-[11px] text-fg-tertiary">{name}</span>
        </span>
        {name === THEUTH ? <Badge variant="outline">built-in</Badge> : a && <ActiveSwitch agent={a} withLabel={false} />}
      </div>
      <div className="flex items-center gap-1.5 text-xs text-fg-secondary">
        {a && (a.file.paused ? <span className="text-fg-tertiary">paused</span> : <ActivityChip status={a.status} />)}
        {a && <span className="ml-auto tabular-nums">{a.open_tasks} open</span>}
      </div>
      {a && <BudgetBar agent={a} />}
    </Link>
  )
}
