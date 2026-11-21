import { Forward, X } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { useQuerySupport } from '@/rpc/capabilities'
import { SnoozeMenu } from './actions'
import { DISMISS_HINT } from './labels'

type Props = {
  /** Items behind the selection (a stack counts every member). */
  count: number
  /** Every selected item is an approval: Allow N / Deny N. */
  approvals: boolean
  pending: boolean
  onDismiss: () => void
  onSnooze: (until: number) => void
  onResolve: (decision: 'allow' | 'deny') => void
  onClear: () => void
}

/** 207 §8.5: the selection's bar under the list — N selected · Dismiss · Snooze ▾ · Allow N / Deny N (approvals only) · Clear. */
export function BulkBar({ count, approvals, pending, onDismiss, onSnooze, onResolve, onClear }: Props) {
  const leaves = useQuerySupport(['attention_dismiss_many'])
  return (
    <div role="toolbar" aria-label="Selection" className="flex shrink-0 flex-col gap-1 border-t border-border-subtle bg-layer-1 px-3 pt-2 pb-[calc(0.5rem+var(--safe-bottom))]">
      <div className="flex flex-wrap items-center gap-1.5">
        <span className="mr-auto text-xs font-medium tabular-nums">{count} selected</span>
        {approvals && (
          <>
            <Button size="xs" disabled={pending} onClick={() => onResolve('allow')}>
              Allow {count}
            </Button>
            <Button size="xs" variant="outline" disabled={pending} onClick={() => onResolve('deny')}>
              Deny {count}
            </Button>
          </>
        )}
        <SnoozeMenu size="xs" disabled={pending} label={`Snooze ${count}`} onPick={onSnooze} />
        <Button size="xs" variant="outline" disabled={pending} title={leaves ? DISMISS_HINT : 'Hide the selected items'} onClick={onDismiss}>
          <Forward />
          Dismiss {count}
        </Button>
        <Button size="icon-xs" variant="ghost" aria-label="Clear selection" onClick={onClear}>
          <X />
        </Button>
      </div>
      {leaves && <p className="text-[11px] text-fg-tertiary">Requests are left to their agents: nothing is refused, no session is woken.</p>}
    </div>
  )
}
