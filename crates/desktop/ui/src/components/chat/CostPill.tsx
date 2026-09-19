import { useQuery } from '@tanstack/react-query'
import { useEffect, useState } from 'react'
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog'
import { costLines, formatMoney } from '@/lib/money'
import { queryKeys, useApi } from '@/lib/rpc'
import type { CostBucket } from '@/rpc/bindings/CostBucket'
import type { SessionCost } from '@/rpc/bindings/SessionCost'

function Bucket({ label, bucket }: { label: string; bucket: CostBucket }) {
  const { primary, secondary } = costLines(bucket)
  return (
    <div className="flex flex-col gap-0.5 tabular-nums">
      <div className="flex justify-between gap-4">
        <span className="text-fg-secondary">{label}</span>
        <span>{primary}</span>
      </div>
      {secondary && <div className="text-right text-xs text-fg-tertiary">{secondary}</div>}
      {bucket.unresolved > 0 && (
        <div className="text-right text-xs text-fg-tertiary">{bucket.unresolved} unresolved attempts</div>
      )}
    </div>
  )
}

function Breakdown({ cost }: { cost: SessionCost }) {
  return (
    <div className="flex flex-col gap-3 text-sm">
      <Bucket label="Direct" bucket={cost.direct} />
      <Bucket label="Descendants" bucket={cost.descendants} />
      <Bucket label="Total" bucket={cost.total} />
      <p className="text-xs text-fg-tertiary">
        {cost.history_coverage === 'complete' ? 'Complete since creation' : 'Historical cost unknown'}
        {cost.scope === 'opaque_remote_inclusive' && ' · includes hidden remote descendants'}
        {cost.remote_meter_gaps > 0 && ` · ${cost.remote_meter_gaps} remote meter gaps`}
      </p>
      {cost.children.length > 0 && (
        <table className="w-full text-xs">
          <thead className="text-left text-fg-tertiary">
            <tr>
              <th className="font-normal">Child</th>
              <th className="text-right font-normal">Inclusive total</th>
            </tr>
          </thead>
          <tbody>
            {cost.children.map((c) => (
              <tr key={c.session}>
                <td className="truncate py-0.5">
                  {c.name ?? c.session}
                  {c.deleted && <span className="ml-1 text-fg-tertiary">(deleted)</span>}
                  {c.scope === 'opaque_remote_inclusive' && <span className="ml-1 text-fg-tertiary">(remote)</span>}
                </td>
                <td className="py-0.5 text-right tabular-nums">{costLines(c.total).primary}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  )
}

/** Header pill with the session's returned total (51 §6.2); click or `/cost` opens the breakdown. */
export function CostPill({ session }: { session: string }) {
  const api = useApi()
  const [open, setOpen] = useState(false)
  const { data } = useQuery({ queryKey: queryKeys.sessionCost(session), queryFn: () => api.session_cost(session) })
  useEffect(() => {
    const h = (e: Event) => {
      if ((e as CustomEvent<{ session: string }>).detail.session === session) setOpen(true)
    }
    window.addEventListener('wth:open-cost', h)
    return () => window.removeEventListener('wth:open-cost', h)
  }, [session])
  if (!data) return null
  const amount = data.total.returned.length ? data.total.returned.map(formatMoney).join(' + ') : '—'
  return (
    <>
      <button
        type="button"
        className="rounded-full border border-border-subtle px-2.5 py-0.5 text-xs whitespace-nowrap text-fg-secondary tabular-nums"
        title={costLines(data.total).primary}
        onClick={() => setOpen(true)}
      >
        <span className="hidden md:inline">Returned cost </span>
        {amount}
        {data.total.unavailable > 0 && <span className="hidden md:inline"> · {data.total.unavailable} unavailable</span>}
      </button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="max-w-md">
          <DialogTitle>Cost</DialogTitle>
          <Breakdown cost={data} />
        </DialogContent>
      </Dialog>
    </>
  )
}
