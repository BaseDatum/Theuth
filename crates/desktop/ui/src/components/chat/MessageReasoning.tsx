import { ChevronRight } from 'lucide-react'
import { memo, useState } from 'react'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Pulse } from './Pulse'

/** `Thinking…` while the model reasons (open), `Thought` once done (collapsed until clicked). */
export const MessageReasoning = memo(function MessageReasoning({ text, done }: { text: string; done: boolean }) {
  const [chosen, setChosen] = useState<boolean | null>(null)
  const open = chosen ?? !done
  return (
    <Collapsible open={open} onOpenChange={setChosen} className="py-1">
      <CollapsibleTrigger className="group flex items-center gap-2 text-sm text-fg-secondary hover:text-foreground">
        {done ? <ChevronRight className="size-4 transition-transform group-data-[state=open]:rotate-90" /> : <Pulse className="size-2" />}
        {done ? 'Thought' : 'Thinking…'}
      </CollapsibleTrigger>
      <CollapsibleContent className="mt-2 border-l-2 border-border-subtle pl-4 text-sm leading-6 text-fg-tertiary whitespace-pre-wrap">
        {text}
      </CollapsibleContent>
    </Collapsible>
  )
})
