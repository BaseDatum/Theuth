import { ChevronRight } from 'lucide-react'
import { memo, useState } from 'react'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Pulse } from './Pulse'

/**
 * A thinking block, collapsed behind a chevron until clicked (70 THINKING-TRACES).
 * While the model still thinks the header pulses and echoes the latest line; the
 * full text streams inside whenever the block is open.
 */
export const MessageReasoning = memo(function MessageReasoning({ text, done }: { text: string; done: boolean }) {
  const [open, setOpen] = useState(false)
  return (
    <Collapsible open={open} onOpenChange={setOpen} className="py-1">
      <CollapsibleTrigger className="group flex w-full min-w-0 items-center gap-2 text-left text-sm text-fg-secondary hover:text-foreground">
        <ChevronRight className="size-4 shrink-0 transition-transform group-data-[state=open]:rotate-90" />
        {done ? (
          'Thought'
        ) : (
          <>
            <Pulse className="size-2 shrink-0" />
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
