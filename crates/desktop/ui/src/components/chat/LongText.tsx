import { useDisclosure } from '@/lib/tool-runs'
import { longText, showAll } from '@/lib/long-text'
import { cn } from '@/lib/utils'
import { CopyButton } from './MessageAssistant'

/**
 * 171F B6: the user's words in their bubble (`className`); a long text (`longText`) folds to eight lines with a bottom
 * fade. The full text stays in the DOM either way (selection, ⌘C, screen readers); the toggle row and the copy button
 * never reach the bubble's timestamp toggle. `disclosure` is the `disclosureKey` that keeps it open across remounts.
 */
export function LongText({ text, className, disclosure }: { text: string; className: string; disclosure: string }) {
  const [open, setOpen] = useDisclosure(disclosure)
  const long = longText(text)
  if (!long) return <div className={className}>{text}</div>
  return (
    <div className={className}>
      <div className={cn(!open && 'line-clamp-8 mask-b-from-60%')}>{text}</div>
      <div className="-mx-2 -mb-1.5 flex items-center justify-between gap-2 whitespace-normal" onClick={(e) => e.stopPropagation()}>
        <button type="button" aria-expanded={open} className="min-h-11 px-2 text-sm text-fg-secondary hover:text-fg-primary" onClick={() => setOpen(!open)}>
          {open ? 'Show less' : showAll(long)}
        </button>
        <CopyButton text={text} />
      </div>
    </div>
  )
}
