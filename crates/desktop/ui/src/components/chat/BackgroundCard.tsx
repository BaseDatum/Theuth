import { ChevronRight } from 'lucide-react'
import type { ReactNode } from 'react'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { cn } from '@/lib/utils'
import { CategoryMark } from './CategoryMark'

/** Body before header: expanding never moves its own disclosure target. `bodyScroll`: the body's height cap and overflow
 * (default a third of the visible band), `''` where the content scrolls itself. */
export function BackgroundCard({ title, summary, color, expanded, onExpanded, bodyScroll = 'max-h-[calc(var(--visible-band,100dvh)/3)] overflow-y-auto', children }: {
  title: string
  summary: ReactNode
  color: string
  expanded: boolean
  onExpanded(open: boolean): void
  bodyScroll?: string
  children: ReactNode
}) {
  return (
    <Collapsible open={expanded} onOpenChange={onExpanded} className="tray-card-collapsible tray-float shrink-0 overflow-hidden rounded-xl border text-xs">
      <CollapsibleContent>
        <div className={cn('border-b px-3 py-2', bodyScroll)}>{children}</div>
      </CollapsibleContent>
      <CollapsibleTrigger className="flex min-h-9 w-full items-center gap-2 px-3 py-1.5 text-left outline-none hover:bg-layer-2 focus-visible:ring-[3px] focus-visible:ring-ring/50">
        <CategoryMark color={color} />
        <span className="shrink-0 font-medium">{title}</span>
        <span className="min-w-0 flex-1 truncate text-fg-secondary tabular-nums">{summary}</span>
        <ChevronRight className={cn('size-3.5 shrink-0 text-fg-tertiary transition-transform', expanded && '-rotate-90')} />
      </CollapsibleTrigger>
    </Collapsible>
  )
}
