import { ChevronRight } from 'lucide-react'
import type { ReactNode } from 'react'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { cn } from '@/lib/utils'

/** Body before header: expanding never moves its own disclosure target. */
export function BackgroundCard({ title, summary, color, expanded, onExpanded, children }: {
  title: string
  summary: ReactNode
  color: string
  expanded: boolean
  onExpanded(open: boolean): void
  children: ReactNode
}) {
  return (
    <Collapsible open={expanded} onOpenChange={onExpanded} className="tray-card-collapsible shrink-0 overflow-hidden rounded-xl border bg-background text-xs">
      <CollapsibleContent>
        <div className="max-h-[calc(var(--visible-band,100dvh)/3)] overflow-y-auto border-b px-3 py-2">{children}</div>
      </CollapsibleContent>
      <CollapsibleTrigger className="flex min-h-9 w-full items-center gap-2 px-3 py-1.5 text-left outline-none hover:bg-layer-2 focus-visible:ring-[3px] focus-visible:ring-ring/50">
        <span className={cn('h-4 w-0.5 shrink-0 rounded-full', color)} />
        <span className="shrink-0 font-medium">{title}</span>
        <span className="min-w-0 flex-1 truncate text-fg-secondary tabular-nums">{summary}</span>
        <ChevronRight className={cn('size-3.5 shrink-0 text-fg-tertiary transition-transform', expanded && '-rotate-90')} />
      </CollapsibleTrigger>
    </Collapsible>
  )
}
