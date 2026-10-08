import NumberFlow from '@number-flow/react'
import { useQuery } from '@tanstack/react-query'
import { Coins, Eye } from 'lucide-react'
import { Fragment, useEffect, useState } from 'react'
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog'
import { costLines, costShort, moneyParts } from '@/lib/money'
import { queryKeys, useApi } from '@/lib/rpc'
import type { CostBucket } from '@/rpc/bindings/CostBucket'
import type { RemoteTokens } from '@/rpc/bindings/RemoteTokens'
import type { ObservedMeta } from '@/rpc/bindings/ObservedMeta'
import type { SessionCost } from '@/rpc/bindings/SessionCost'
import { useSessionField } from '@/store/sessions'
import { MenuRow } from './HeaderMenu'

/** 141 §7.6: the one-line rule a watch-only child's breakdown names — why its money is, or is not, in the totals. */
export const COST_RULE: Record<ObservedMeta['cost_rule'], string> = {
  counted: 'counted — omp background subagent, not in the harness’s own total',
  inside_parent: 'inside parent — omp sync subagent, already in the harness’s total',
  display_only: 'reported by the harness, not counted',
  unreported: 'not reported by the harness',
}

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

/** `12.3k`, `200k`, `1.0M`: the TUI's `k`, so `/cost` reads the same on both surfaces. */
export function k(n: number): string {
  const t = n / 1000
  if (n < 1000) return String(n)
  if (t < 100) return `${t.toFixed(1)}k`
  if (t < 1000) return `${t.toFixed(0)}k`
  return `${(t / 1000).toFixed(1)}M`
}

/** How many harness rows show before `Show more`. */
const HARNESS_CAP = 5

/**
 * ACP children's tokens per harness model (75 §7, §13 row 6). Raw as the harness sent them; a `lower_bound` group
 * (opencode 1.x: last request only, #1860) says so (06 §8.4).
 */
export function HarnessTokens({ rows }: { rows: RemoteTokens[] }) {
  const [expanded, setExpanded] = useState(false)
  if (!rows.length) return null
  const shown = expanded ? rows : rows.slice(0, HARNESS_CAP)
  const hidden = rows.length - shown.length
  return (
    <div className="flex flex-col gap-1">
      <div className="text-fg-secondary">Harness tokens</div>
      <table className="w-full text-xs tabular-nums">
        <thead className="text-left text-fg-tertiary">
          <tr>
            <th className="font-normal">Model</th>
            <th className="text-right font-normal">Turns</th>
            <th className="text-right font-normal">In</th>
            <th className="text-right font-normal">Out</th>
            <th className="text-right font-normal">Cache read</th>
            <th className="text-right font-normal">Cache write</th>
          </tr>
        </thead>
        <tbody>
          {shown.map((r) => (
            <tr key={`${r.harness}\0${r.model ?? ''}`}>
              <td className="truncate py-0.5">
                {r.harness}
                {r.model && <span className="text-fg-tertiary"> · {r.model}</span>}
                {r.lower_bound && <span className="ml-1 text-fg-tertiary">(last request only)</span>}
              </td>
              <td className="py-0.5 text-right">{r.turns}</td>
              <td className="py-0.5 text-right">{k(r.tokens.input)}</td>
              <td className="py-0.5 text-right">{k(r.tokens.output)}</td>
              <td className="py-0.5 text-right">{k(r.tokens.cache_read)}</td>
              <td className="py-0.5 text-right">{k(r.tokens.cache_write)}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {(hidden > 0 || expanded) && (
        <button type="button" className="self-start text-xs text-fg-tertiary hover:text-fg" onClick={() => setExpanded((v) => !v)}>
          {expanded ? 'Show less' : `Show more (${hidden})`}
        </button>
      )}
    </div>
  )
}

/** `observed`: the session's own watch-only facts (141), when it is a watch-only child — adds the cost-rule line. */
export function Breakdown({ cost, observed }: { cost: SessionCost; observed?: ObservedMeta | null }) {
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
      {cost.delta_suspected && <p className="text-xs text-fg-tertiary">harness may report per-turn cost — totals may be low</p>}
      {observed && <p className="text-xs text-fg-tertiary">watch-only: {COST_RULE[observed.cost_rule]}</p>}
      <HarnessTokens rows={cost.remote_tokens} />
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
                  {c.scope === 'observed' && <Eye className="mr-1 inline size-3 opacity-70" aria-label="watch-only" role="img" />}
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

/** Three fixed decimals, no grouping: `formatMoney`'s digits. */
const MILLI = { minimumFractionDigits: 3, maximumFractionDigits: 3, useGrouping: false }

/** `costShort` with rolling digits: returned money, else `~` estimates, joined ` + `; `—` stays text. Reads the same at rest. */
function RollingCost({ bucket }: { bucket: CostBucket }) {
  const monies = bucket.returned.length ? bucket.returned : bucket.estimated_for_unavailable
  if (!monies.length) return '—'
  return monies.map((m, i) => {
    const { prefix, value } = moneyParts(m)
    return (
      <Fragment key={i}>
        {i > 0 && ' + '}
        {!bucket.returned.length && '~'}
        <NumberFlow value={value} prefix={prefix} locales="en" format={MILLI} />
      </Fragment>
    )
  })
}

/** Header pill with the session's returned total, else its estimate (51 §6.2); `row`: the compact menu's line. Click opens `CostDialog`. */
export function CostPill({ session, row, onOpen }: { session: string; row?: boolean; onOpen(): void }) {
  const data = useCost(session)
  if (!data) return null
  const lines = costLines(data.total)
  if (row) return <MenuRow icon={Coins} label="Cost" detail={costShort(data.total)} onSelect={onOpen} />
  return (
    <button
      type="button"
      className="rounded-full border border-border-subtle px-2.5 py-0.5 text-xs whitespace-nowrap text-fg-secondary tabular-nums"
      title={lines.secondary ? `${lines.primary}\n${lines.secondary}` : lines.primary}
      aria-label="Cost"
      onClick={onOpen}
    >
      <RollingCost bucket={data.total} />
    </button>
  )
}

/** The breakdown; lives in the header (not the pill) so it outlives a compact menu that closes on selection. `/cost` opens it too. */
export function CostDialog({ session, open, onOpenChange }: { session: string; open: boolean; onOpenChange(o: boolean): void }) {
  const data = useCost(session)
  const observed = useSessionField(session, 'observed')
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
        <Breakdown cost={data} observed={observed} />
      </DialogContent>
    </Dialog>
  )
}
