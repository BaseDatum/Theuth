import { ArrowLeft } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { backToSearch, useSearchOrigin } from '@/lib/search'
import { cn } from '@/lib/utils'

/** Back to results (49 §7): shown only on a screen search opened; destination owners gate on `isSearchDestination` where a screen can outlive its origin. */
export function SearchReturn({ className }: { className?: string }) {
  const origin = useSearchOrigin()
  if (!origin) return null
  return (
    <Button variant="ghost" size="sm" className={cn('gap-1 text-fg-secondary', className)} onClick={backToSearch}>
      <ArrowLeft className="size-4" />
      Back to results
    </Button>
  )
}
