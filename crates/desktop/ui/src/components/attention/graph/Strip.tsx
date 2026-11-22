// 208 §7.1: the graph region's 32 px bar — collapsed it is the whole region, expanded it heads the canvas and carries
// its controls. No `@xyflow/react` import.
import { ChevronDown, X } from 'lucide-react'
import type { ReactNode } from 'react'
import { cn } from '@/lib/utils'
import type { GraphOlder } from '@/rpc/bindings/method.agent.output'
import { type StripCounts, stripText } from './selectors'

export type StripProps = {
  counts: StripCounts
  older: GraphOlder | null
  /** The last engine call failed while an earlier state is shown (review U4). */
  stale?: boolean
  /** The list's subtree filter label, mirrored here. */
  subtree: string | null
  expanded: boolean
  onToggle: () => void
  onClearSubtree?: () => void
  /** Right-hand controls while expanded. */
  children?: ReactNode
}

export function Strip({ counts, older, stale = false, subtree, expanded, onToggle, onClearSubtree, children }: StripProps) {
  return (
    <div className="flex h-8 shrink-0 items-center gap-2 border-b border-border-subtle px-2 text-xs text-fg-secondary">
      <button
        type="button"
        aria-expanded={expanded}
        aria-keyshortcuts="G"
        title={expanded ? 'Collapse the agent graph (G)' : 'Expand the agent graph (G)'}
        onClick={onToggle}
        className="flex h-6 min-w-0 items-center gap-1.5 rounded-md px-1 hover:bg-layer-2"
      >
        <ChevronDown className={cn('size-3.5 shrink-0 text-fg-tertiary motion-safe:transition-transform', !expanded && '-rotate-90')} />
        <span className="font-medium text-foreground">Agents</span>
        <span className={cn('truncate tabular-nums', counts.needYou > 0 && 'text-danger')}>{stripText(counts)}</span>
      </button>
      {stale && (
        <span className="shrink-0 text-fg-tertiary" title="The last update from the engine failed; retrying">
          stale
        </span>
      )}
      {older && older.roots > 0 && (
        <span className="hidden shrink-0 text-fg-tertiary tabular-nums sm:inline" title={`${older.nodes} sessions in idle trees left out`}>
          +{older.roots} older trees
        </span>
      )}
      {subtree !== null && (
        <span className="flex h-5 max-w-40 shrink-0 items-center gap-1 rounded-full border border-fg-accent bg-fg-accent/10 pr-0.5 pl-2 text-foreground">
          <span className="truncate">In {subtree}</span>
          {onClearSubtree && (
            <button type="button" aria-label="Clear tree filter" onClick={onClearSubtree} className="rounded-full p-0.5 text-fg-tertiary hover:text-foreground">
              <X className="size-3" />
            </button>
          )}
        </span>
      )}
      <div className="ml-auto flex shrink-0 items-center gap-0.5">{children}</div>
    </div>
  )
}
