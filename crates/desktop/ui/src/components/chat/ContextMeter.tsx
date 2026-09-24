import { useQuery } from '@tanstack/react-query'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { useSessionField } from '@/store/sessions'

const k = (n: number) => `${Math.round(n / 1000)}k`

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
    <span
      className={cn(
        'rounded-full border px-2.5 py-0.5 text-xs whitespace-nowrap tabular-nums',
        pct === null || pct < 70
          ? 'border-border-subtle text-fg-secondary'
          : pct < 90
            ? 'border-warning-border bg-warning-bg text-warning'
            : 'border-danger-border bg-danger-bg text-danger',
      )}
      title={`${estimated ? 'Estimated context window used' : 'Context window used'}${compact ? '' : ' · last turn prefix cache hit rate'}`}
    >
      {estimated && '~'}
      {pct === null ? k(used) : compact ? `${pct}%` : `${k(used)}/${k(limit)} · ${pct}%`}
      {cache !== null && !compact && ` · cache ${cache}%`}
    </span>
  )
}
