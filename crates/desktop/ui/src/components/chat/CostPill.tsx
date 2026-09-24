import { useQuery } from '@tanstack/react-query'
import { Coins } from 'lucide-react'
import { useEffect } from 'react'
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog'
import { costLines, costShort } from '@/lib/money'
import { queryKeys, useApi } from '@/lib/rpc'
import type { CostBucket } from '@/rpc/bindings/CostBucket'
import type { SessionCost } from '@/rpc/bindings/SessionCost'
import { MenuRow } from './HeaderMenu'

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

const useCost = (session: string) => {
  const api = useApi()
  return useQuery({ queryKey: queryKeys.sessionCost(session), queryFn: () => api.session_cost(session) }).data
}

/** Header pill with the session's returned total, else its estimate (51 §6.2); `row`: the compact menu's line. Click opens `CostDialog`. */
export function CostPill({ session, row, onOpen }: { session: string; row?: boolean; onOpen(): void }) {
  const data = useCost(session)
  if (!data) return null
  const amount = costShort(data.total)
  const lines = costLines(data.total)
  if (row) return <MenuRow icon={Coins} label="Cost" detail={amount} onSelect={onOpen} />
  return (
    <button
      type="button"
      className="rounded-full border border-border-subtle px-2.5 py-0.5 text-xs whitespace-nowrap text-fg-secondary tabular-nums"
      title={lines.secondary ? `${lines.primary}\n${lines.secondary}` : lines.primary}
      aria-label="Cost"
      onClick={onOpen}
    >
      {amount}
    </button>
  )
}

/** The breakdown; lives in the header (not the pill) so it outlives a compact menu that closes on selection. `/cost` opens it too. */
export function CostDialog({ session, open, onOpenChange }: { session: string; open: boolean; onOpenChange(o: boolean): void }) {
  const data = useCost(session)
  useEffect(() => {
    const h = (e: Event) => {
      if ((e as CustomEvent<{ session: string }>).detail.session === session) onOpenChange(true)
    }
    window.addEventListener('theuth:open-cost', h)
    return () => window.removeEventListener('theuth:open-cost', h)
  }, [session, onOpenChange])
  if (!data) return null
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md">
        <DialogTitle>Cost</DialogTitle>
        <Breakdown cost={data} />
      </DialogContent>
    </Dialog>
  )
}
