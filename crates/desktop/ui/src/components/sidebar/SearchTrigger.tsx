import { Search } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { label } from '@/lib/chord'
import { useShortcuts } from '@/lib/shortcuts'
import { cn } from '@/lib/utils'

/**
 * Opens the one root-mounted search (49 §5.1) by dispatching `theuth:search`. Ghost icon: 36 px under a mouse,
 * 44 px under a finger. `data-search-trigger` lets the root hand focus back here after a dismissed drawer
 * unmounted the trigger that opened search.
 */
export function SearchTrigger({ className }: { className?: string }) {
  const search = useShortcuts((s) => s.chords.search)
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          data-search-trigger=""
          className={cn('rounded-lg text-fg-secondary', className)}
          aria-label="Search"
          onClick={() => window.dispatchEvent(new Event('theuth:search'))}
        >
          <Search className="size-[18px]" />
        </Button>
      </TooltipTrigger>
      <TooltipContent>Search ({label(search)})</TooltipContent>
    </Tooltip>
  )
}
