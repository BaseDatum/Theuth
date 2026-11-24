import { cn } from '@/lib/utils'
import { useItemError } from './queries'

/** A failed action on an inbox item, inline under it — the Attention window never toasts (`setItemError`). */
export function ItemError({ ids, className }: { ids: readonly string[]; className?: string }) {
  const error = useItemError(ids)
  if (error === null) return null
  return (
    <p role="alert" className={cn('whitespace-pre-line text-xs text-danger', className)}>
      {error}
    </p>
  )
}
