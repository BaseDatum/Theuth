import { badgeCount } from './model'
import { useAttention } from './queries'

/** The sidebar's Attention count: undismissed, unsnoozed items; hidden at zero (70 HUD). */
export function AttentionBadge() {
  const n = badgeCount(useAttention().data)
  if (n === null) return null
  return (
    <span className="ml-auto rounded-full bg-warning px-1.5 text-[11px] font-semibold tabular-nums text-primary-foreground" aria-label={`${n} items in the inbox`}>
      {n}
    </span>
  )
}
