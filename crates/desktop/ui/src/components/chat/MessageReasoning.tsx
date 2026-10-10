import { ChevronRight } from 'lucide-react'
import { memo } from 'react'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { fmtThought } from '@/lib/time'
import { useToolRuns } from '@/lib/tool-runs'
import { ThinkingIndicator } from './Indicators'

/**
 * A thinking block, collapsed behind a chevron until clicked (70 THINKING-TRACES).
 * While the model still thinks the header pulses and echoes the latest line; the
 * full text streams inside whenever the block is open. Once done the header says
 * how long it took — "Thought 9s" — when the time is known (91 THOUGHT-TIME).
 *
 * The open state lives in `useToolRuns.thinking` keyed by the entry id, never in the component: a row remounts when
 * it folds into a tool run, moves between the transcript's urgent and deferred slots, or reloads, and each of those
 * closed a `useState`. Opening while it still streams pins it so the next tool call cannot fold it away (D2).
 */
export const MessageReasoning = memo(function MessageReasoning({ id, text, done, ms }: { id: string; text: string; done: boolean; ms?: number }) {
  const open = useToolRuns((s) => s.thinking[id] ?? false)
  const toggle = useToolRuns((s) => s.toggleThinking)
  const took = done && ms !== undefined ? fmtThought(ms) : undefined
  return (
    <Collapsible open={open} onOpenChange={() => toggle(id, !done)} className="py-1">
      <CollapsibleTrigger className="group flex w-full min-w-0 items-center gap-2 text-left text-sm text-fg-secondary hover:text-foreground">
        <ChevronRight className="size-4 shrink-0 transition-transform group-data-[state=open]:rotate-90" />
        {done ? (
          <>
            Thought
            {took && <span className="text-fg-tertiary">{took}</span>}
          </>
        ) : (
          <>
            <ThinkingIndicator />
            <span className="shrink-0">Thinking…</span>
            {!open && <span className="truncate text-fg-tertiary">{text.trimEnd().split('\n').pop()}</span>}
          </>
        )}
      </CollapsibleTrigger>
      <CollapsibleContent className="mt-2 border-l-2 border-border-subtle pl-4 text-sm leading-6 text-fg-tertiary whitespace-pre-wrap">
        {text}
      </CollapsibleContent>
    </Collapsible>
  )
})
