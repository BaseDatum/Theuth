// 208 §7.2: one graph node's card at three levels of detail. Tone, glyph and kind come from encoding.ts (shared with the
// minimap, the edges and the legend). No `@xyflow/react` import: the canvas wraps it, the tests render it statically.
import { Check, Hourglass, Square, TriangleAlert, X } from 'lucide-react'
import { createContext, type CSSProperties, memo } from 'react'
import { EyeMark } from '@/components/chat/ObservedBar'
import { counts } from '@/components/chat/broadcast'
import { ago } from '@/components/sidebar/AgentTree'
import { HarnessGlyph, shortModel } from '@/components/sidebar/harnessGlyph'
import { cn } from '@/lib/utils'
import type { GraphNode } from '@/rpc/bindings/method.agent.output'
import { type GlyphKey, KIND, MARK, STATUS, statusOf } from './encoding'
import { nodeLabel, statusText } from './selectors'

export type Lod = 'full' | 'compact' | 'dot'
/** The canvas's current level of detail (nodes and edge labels read it). */
export const LodContext = createContext<Lod>('full')
export type NodeBodyProps = { node: GraphNode; lod: Lod; attention: number; highlighted: boolean; selected: boolean; opacity: number; now: number }

/** A status glyph drawn in `currentColor` (the caller sets the tone). */
export function StatusIcon({ glyph, className }: { glyph: GlyphKey; className?: string }) {
  const c = cn('size-full', className)
  switch (glyph) {
    case 'pulse':
      return <span className={cn('block size-[55%] rounded-full bg-current motion-safe:animate-pulse', className)} />
    case 'alert':
      return <TriangleAlert className={c} strokeWidth={2.5} />
    case 'hourglass':
      return <Hourglass className={c} strokeWidth={2.5} />
    case 'cross':
      return <X className={c} strokeWidth={3} />
    case 'check':
      return <Check className={c} strokeWidth={3} />
    case 'square':
      return <Square className={c} strokeWidth={0} fill="currentColor" />
    case 'ring':
      return <span className={cn('block size-[55%] rounded-full border-[1.5px] border-current', className)} />
  }
}

/** The status chip: the glyph on a tinted disc in the status tone (readable on both themes' cards). */
export function StatusChip({ glyph, tone, className }: { glyph: GlyphKey; tone: string; className?: string }) {
  return (
    <span className={cn('flex shrink-0 items-center justify-center rounded-full p-[3px]', className)} style={{ color: tone, backgroundColor: `color-mix(in oklab, ${tone} 16%, transparent)` }}>
      <StatusIcon glyph={glyph} />
    </span>
  )
}

/** The danger badge of a node somebody is waiting on: the count of its open attention items. */
function Badge({ n, className }: { n: number; className?: string }) {
  return <span className={cn('shrink-0 rounded-full bg-danger px-1.5 text-[10px] leading-4 font-semibold text-white tabular-nums shadow-xs', className)}>{n}</span>
}

/** The card's second row: status · model · jobs · age. */
export function nodeDetail(node: GraphNode, now: number): string {
  const model = shortModel(node.model)
  const jobs = node.activity.jobs > 0 || node.activity.ptys > 0 ? counts(node.activity) : ''
  const when = ago(new Date(node.last_event_ms).toISOString(), now)
  return [statusText(node), model, jobs, when].filter((x) => x !== '').join(' · ')
}

export const NodeBody = memo(function NodeBody({ node, lod, attention, highlighted, selected, opacity, now }: NodeBodyProps) {
  const label = nodeLabel(node)
  const status = statusText(node)
  const st = STATUS[statusOf(node)]
  const kind = KIND[node.kind]
  const top = node.kind === 'root' || node.kind === 'desk'
  const ring = attention > 0
  const ghost = opacity <= MARK.ghost.opacity
  const lit = selected || highlighted
  const tone = { '--node-tone': st.tone } as CSSProperties
  const frame = cn(
    'ag-node relative flex h-16 w-[200px] overflow-hidden rounded-[10px] border text-left text-foreground',
    'shadow-[0_1px_2px_rgb(0_0_0/0.06),0_2px_6px_-2px_rgb(0_0_0/0.08)] transition-[border-color,box-shadow] duration-150',
    top ? 'border-border-strong bg-elevated' : 'border-border-subtle bg-layer-1',
    node.kind === 'peer' && 'border-dashed border-border-strong bg-background',
    ghost && 'border-dashed',
    lit && 'border-fg-accent',
    selected && 'shadow-[0_0_0_1.5px_var(--color-fg-accent),0_4px_14px_-4px_rgb(0_0_0/0.25)]',
    ring && 'ring-2 ring-danger ring-offset-2 ring-offset-background',
  )
  const common = { 'data-node-id': node.id, 'data-lod': lod, 'data-status': statusOf(node), 'aria-label': `${label} · ${status}` }
  // The status strip down the card's left edge: the tone at a glance at any zoom.
  const strip = <span aria-hidden className="absolute inset-y-0 left-0 w-[3px] bg-[var(--node-tone)]" />
  if (lod === 'dot')
    // Far out the whole box is one block in the status tone (edges still meet it); the ring is drawn thick enough to
    // survive the zoom.
    return (
      <div
        {...common}
        style={{ ...tone, backgroundColor: node.kind === 'peer' ? 'transparent' : `color-mix(in oklab, ${st.tone} ${st.glyph === 'ring' ? 45 : 85}%, transparent)` }}
        className={cn(
          'flex h-16 w-[200px] rounded-2xl border-4 border-transparent',
          top && 'border-[var(--node-tone)]',
          node.kind === 'peer' && 'border-dashed border-fg-tertiary',
          lit && 'border-fg-accent',
          ring && 'ring-8 ring-danger ring-offset-4 ring-offset-background',
        )}
      />
    )
  if (lod === 'compact')
    return (
      // Compact runs at 0.4–0.75 zoom: the name is drawn large (two lines before it truncates) so it reads at 9–16 px.
      <div {...common} style={tone} className={cn(frame, 'items-center gap-2 pr-2.5 pl-3')}>
        {strip}
        <StatusChip glyph={st.glyph} tone={st.tone} className="size-7 p-1.5" />
        <span className={cn('line-clamp-2 min-w-0 flex-1 text-[19px] leading-[1.15] break-words', top ? 'font-semibold' : 'font-medium')}>{label}</span>
        {ring && <Badge n={attention} className="px-2 text-base leading-6" />}
      </div>
    )
  const row2 = nodeDetail(node, now)
  const failed = st === STATUS.failed
  return (
    <div {...common} style={tone} className={cn(frame, 'flex-col justify-center gap-[3px] pr-2 pl-3')}>
      {strip}
      <div className="flex min-w-0 items-center gap-1.5">
        <StatusChip glyph={st.glyph} tone={st.tone} className="size-4" />
        {node.agent && <HarnessGlyph agent={node.agent} className="size-3.5 shrink-0 text-fg-secondary" />}
        {node.observed && <EyeMark />}
        <span className={cn('min-w-0 flex-1 truncate text-[13px] leading-[18px] tracking-[-0.005em]', top ? 'font-semibold' : 'font-medium')}>{label}</span>
        {ring && <Badge n={attention} />}
      </div>
      <div className="flex min-w-0 items-center gap-1.5 text-[11px] leading-4 text-fg-secondary">
        <span className={cn('min-w-0 flex-1 truncate tabular-nums', failed && 'text-danger')}>{row2}</span>
        {node.remote && node.kind !== 'peer' ? (
          <span className="shrink-0 rounded-sm bg-layer-2 px-1 text-[10px] text-fg-secondary">remote · {node.owner}</span>
        ) : (
          kind.tag && <span className="shrink-0 rounded-sm border border-border-subtle px-1 text-[9px] leading-[14px] font-medium tracking-wide text-fg-tertiary uppercase">{kind.tag}</span>
        )}
      </div>
    </div>
  )
})
