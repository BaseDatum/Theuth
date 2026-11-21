// 208 §7.2: one graph node's content at three levels of detail, with the sidebar's indicators reused as they are.
// No `@xyflow/react` import: the canvas wraps it, the tests render it statically.
import { memo } from 'react'
import { EyeMark, WatchDot } from '@/components/chat/ObservedBar'
import { counts } from '@/components/chat/broadcast'
import { GLYPH, ago, stateTone } from '@/components/sidebar/AgentTree'
import { HarnessGlyph, shortModel } from '@/components/sidebar/harnessGlyph'
import { cn } from '@/lib/utils'
import type { GraphNode } from '@/rpc/bindings/method.agent.output'
import { nodeLabel, statusText } from './selectors'

export type Lod = 'full' | 'compact' | 'dot'
export type NodeBodyProps = { node: GraphNode; lod: Lod; attention: number; highlighted: boolean; selected: boolean; opacity: number; now: number }

/** The state glyph in the sidebar's tone; a watch-only child shows its harness-state dot. Pulses only with motion allowed. */
function Glyph({ node, className }: { node: GraphNode; className?: string }) {
  if (node.observed) return <WatchDot state={node.observed.state} className={className} />
  return <span className={cn('flex shrink-0 justify-center', stateTone(node.activity.state), 'motion-reduce:animate-none', className)}>{GLYPH[node.activity.state.state]}</span>
}

/** The danger badge of a node somebody is waiting on: the count of its open attention items. */
function Badge({ n, className }: { n: number; className?: string }) {
  return <span className={cn('shrink-0 rounded-full bg-danger px-1.5 text-[10px] leading-4 font-semibold text-white tabular-nums', className)}>{n}</span>
}

/** Dot tone: the glyph's colour as a fill (the minimap uses the same classes). */
export function dotTone(node: GraphNode): string {
  const s = node.activity.state.state
  if (s === 'running') return 'bg-project-malachite'
  if (s === 'approval' || s === 'awaiting') return 'bg-warning'
  if (s === 'error' || node.outcome === 'failed') return 'bg-danger'
  return 'bg-fg-tertiary/45'
}

export const NodeBody = memo(function NodeBody({ node, lod, attention, highlighted, selected, opacity, now }: NodeBodyProps) {
  const label = nodeLabel(node)
  const status = statusText(node)
  const ring = attention > 0
  const ghost = opacity <= 0.25
  const frame = cn(
    'relative flex h-16 w-[200px] flex-col justify-center rounded-lg border bg-layer-1 text-left shadow-xs',
    node.kind === 'peer' ? 'border-dashed border-border-strong' : 'border-border-subtle',
    ghost && 'border-dashed',
    (selected || highlighted) && 'border-fg-accent',
    selected && 'shadow-[0_0_0_1px_var(--color-fg-accent)]',
    ring && 'ring-2 ring-danger',
  )
  const common = { 'data-node-id': node.id, 'data-lod': lod, 'aria-label': `${label} · ${status}` }
  if (lod === 'dot')
    // Far out the whole box is one block in the state tone (edges still meet it); the ring is drawn thick enough to
    // survive the zoom.
    return (
      <div
        {...common}
        className={cn(
          'flex h-16 w-[200px] rounded-xl border-4 border-transparent',
          dotTone(node),
          node.kind === 'peer' && 'border-dashed border-fg-tertiary bg-transparent',
          (selected || highlighted) && 'border-fg-accent',
          ring && 'ring-8 ring-danger ring-offset-4 ring-offset-background',
        )}
      />
    )
  if (lod === 'compact')
    return (
      // Compact runs at 0.4–0.75 zoom: the name is drawn large so it still reads at 10–17 px on screen.
      <div {...common} className={cn(frame, 'flex-row items-center gap-2 px-3')}>
        <Glyph node={node} className="w-6 text-xl [&_svg]:size-5" />
        <span className="min-w-0 flex-1 truncate text-2xl font-medium">{label}</span>
        {ring && <Badge n={attention} className="px-2 text-base leading-6" />}
      </div>
    )
  const model = shortModel(node.model)
  const jobs = node.activity.jobs > 0 || node.activity.ptys > 0 ? counts(node.activity) : ''
  const when = ago(new Date(node.last_event_ms).toISOString(), now)
  const row2 = [status, model, jobs, when].filter((x) => x !== '').join(' · ')
  return (
    <div {...common} className={cn(frame, 'gap-1 px-2.5')} title={`${label} · ${row2}`}>
      <div className="flex min-w-0 items-center gap-1.5 text-[13px]">
        <Glyph node={node} className="w-3.5 text-xs" />
        {node.agent && <HarnessGlyph agent={node.agent} className="size-3.5 shrink-0 text-fg-secondary" />}
        {node.observed && <EyeMark />}
        <span className="min-w-0 flex-1 truncate font-medium">{label}</span>
        {ring && <Badge n={attention} />}
      </div>
      <div className="flex min-w-0 items-center gap-1.5 text-[11px] text-fg-tertiary">
        <span className={cn('min-w-0 flex-1 truncate tabular-nums', (node.activity.state.state === 'error' || node.outcome === 'failed') && 'text-danger')}>{row2}</span>
        {node.remote && node.kind !== 'peer' && <span className="shrink-0 rounded-sm bg-layer-2 px-1 text-[10px] text-fg-secondary">remote · {node.owner}</span>}
      </div>
    </div>
  )
})
