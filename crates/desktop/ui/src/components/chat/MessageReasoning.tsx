import { ChevronRight } from 'lucide-react'
import { memo, useState } from 'react'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { fmtThought } from '@/lib/time'
import { ThinkingIndicator } from './Indicators'

/**
 * A thinking block, collapsed behind a chevron until clicked (70 THINKING-TRACES).
 * While the model still thinks the header pulses and echoes the latest line; the
 * full text streams inside whenever the block is open. Once done the header says
 * how long it took — "Thought 9s" — when the time is known (91 THOUGHT-TIME).
 */
export const MessageReasoning = memo(function MessageReasoning({ text, done, ms }: { text: string; done: boolean; ms?: number }) {
  const [open, setOpen] = useState(false)
  const took = done && ms !== undefined ? fmtThought(ms) : undefined
  return (
    <Collapsible open={open} onOpenChange={setOpen} className="py-1">
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
