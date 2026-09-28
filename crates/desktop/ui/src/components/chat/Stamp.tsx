import { type MouseEvent, useState } from 'react'
import { usePointerCoarse } from '@/lib/mobile'
import { formatAt, fullAt } from '@/lib/time'
import { cn } from '@/lib/utils'

/**
 * A message's wall-clock time (74 §5), shown while the `group` ancestor is hovered on a fine pointer; on a coarse
 * one it has no hover, so the message's tap toggles it (`useStampToggle`). Nothing when the row has no time.
 * Space is reserved by the action row it sits in, so appearing never moves layout.
 */
export function Stamp({ at, shown, className }: { at?: number; shown: boolean; className?: string }) {
  const coarse = usePointerCoarse()
  if (at === undefined) return null
  return (
    <span
      title={fullAt(at)}
      className={cn(
        'text-xs text-fg-tertiary tabular-nums transition-opacity',
        coarse ? (shown ? 'opacity-100' : 'opacity-0') : 'opacity-0 group-hover:opacity-100',
        className,
      )}
    >
      {formatAt(at)}
    </span>
  )
}

/** Coarse-pointer toggle for {@link Stamp}: a tap on the message (not on its controls) shows or hides the time. */
export function useStampToggle(): { shown: boolean; onClick?: (e: MouseEvent) => void } {
  const coarse = usePointerCoarse()
  const [shown, setShown] = useState(false)
  if (!coarse) return { shown }
  return {
    shown,
    onClick: (e) => {
      if ((e.target as Element).closest('button, a, input, textarea')) return
      setShown((v) => !v)
    },
  }
}
