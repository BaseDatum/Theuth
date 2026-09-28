import NumberFlow, { NumberFlowGroup } from '@number-flow/react'
import { useQuery } from '@tanstack/react-query'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { useSessionField } from '@/store/sessions'

/** Rounded thousands, no grouping (`1000k`, never `1,000k`). */
const K = { maximumFractionDigits: 0, useGrouping: false }
/** `${Math.round(n / 1000)}k`, digits rolling; the group makes every number in the pill roll together. */
const k = (n: number) => <NumberFlow value={n / 1000} suffix="k" format={K} />

/**
 * `used/limit k · pct% · cache` pill (colour steps at 70 / 90 %). `compact`: the percent alone, for the narrow header;
 * the pane's own width decides that, not the viewport, so no `md:` gating here.
 */
export function ContextMeter({ session, compact }: { session: string; compact?: boolean }) {
  const api = useApi()
  const used = useSessionField(session, 'context')
  const estimated = useSessionField(session, 'contextEstimated')
  const usage = useSessionField(session, 'usage')
  const model = useSessionField(session, 'model')
  const { data: models } = useQuery({ queryKey: queryKeys.models, queryFn: () => api.list_models() })
  if (used === undefined) return null
  const limit = models?.find((m) => m.id === model)?.context ?? 0
  const pct = limit > 0 ? Math.round((used / limit) * 100) : null
  const cache = usage && usage.prompt > 0 ? Math.round((usage.cached / usage.prompt) * 100) : null
  return (
    <NumberFlowGroup>
      <span
        className={cn(
          'rounded-full border px-2.5 py-0.5 text-xs whitespace-nowrap tabular-nums',
          pct === null || pct < 70
            ? 'border-border-subtle text-fg-secondary'
            : pct < 90
              ? 'border-warning-border bg-warning-bg text-warning'
              : 'border-danger-border bg-danger-bg text-danger',
        )}
        title={`${estimated ? 'Estimated context (≈) until the provider reports' : 'Context window used'}${compact ? '' : ' · last turn prefix cache hit rate'}`}
      >
        {estimated && '≈'}
        {pct === null ? (
          k(used)
        ) : compact ? (
          <NumberFlow value={pct} suffix="%" />
        ) : (
          <>
            {k(used)}/{k(limit)}
            {' · '}
            <NumberFlow value={pct} suffix="%" />
          </>
        )}
        {cache !== null && !compact && (
          <>
            {' · cache '}
            <NumberFlow value={cache} suffix="%" />
          </>
        )}
      </span>
    </NumberFlowGroup>
  )
}
