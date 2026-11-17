import { useQuerySupport } from '@/rpc/capabilities'
import NumberFlow, { NumberFlowGroup } from '@number-flow/react'
import { useQuery } from '@tanstack/react-query'
import { Gauge } from 'lucide-react'
import { queryKeys, useApi } from '@/lib/rpc'
import { detailOf } from '@/lib/session-tree'
import { cn } from '@/lib/utils'
import type { SessionSummary } from '@/rpc/bindings/method.agent.output'
import { useSessionField } from '@/store/sessions'
import { MenuRow } from './HeaderMenu'

/** Rounded thousands, no grouping (`1000k`, never `1,000k`). */
const K = { maximumFractionDigits: 0, useGrouping: false }
/** `${Math.round(n / 1000)}k`, digits rolling; the group makes every number in the pill roll together. */
const k = (n: number) => <NumberFlow value={n / 1000} suffix="k" format={K} />

/** 75 §5 bands: < 75 % normal, 75–90 warning, 90–95 danger, > 95 critical (danger, pulsing). */
export function band(pct: number | null): 'normal' | 'warning' | 'danger' | 'critical' {
  if (pct === null || pct < 75) return 'normal'
  if (pct < 90) return 'warning'
  return pct > 95 ? 'critical' : 'danger'
}

const BAND = {
  normal: ['border-border-subtle text-fg-secondary', ''],
  warning: ['border-warning-border bg-warning-bg text-warning', ' · context filling up'],
  danger: ['border-danger-border bg-danger-bg text-danger', ' · start a new session or summarise'],
  critical: ['border-danger-border bg-danger-bg text-danger animate-pulse', ' · next prompt may fail'],
} as const

/**
 * `used/limit k · pct% · cache` pill (bands at 75 / 90 / 95 %, 75 §5). `limit` is the harness's window when it
 * reported one, the catalog's otherwise. `compact`: the percent alone, for the narrow header;
 * the pane's own width decides that, not the viewport, so no `md:` gating here. `row`: the compact menu's line.
 * Nothing for an `endless` (autocontext) session, chat or desk (Sky 2026-11-17).
 */
/**
 * 141 R3 T14 (Q3): what the meter reads — a watch-only omp child's own `contextTokens/contextWindow` when the harness
 * reported them (a real figure, never `≈`), else the session's context, estimate flag and stored size.
 */
export function meterSource(
  observed: Parameters<typeof detailOf>[0],
  context: number | undefined,
  estimated: boolean | undefined,
  size: number | undefined,
): { used: number | undefined; estimated: boolean; limit: number | undefined } {
  const d = detailOf(observed)
  if (d.contextTokens !== undefined) return { used: d.contextTokens, estimated: false, limit: d.contextWindow ?? size }
  return { used: context, estimated: !!estimated, limit: size }
}

export function ContextMeter({ session, memory, compact, row }: { session: string; memory?: SessionSummary['memory']; compact?: boolean; row?: boolean }) {
  const canQuery1 = useQuerySupport(["list_models"])
  const api = useApi()
  const context = useSessionField(session, 'context')
  const contextEstimated = useSessionField(session, 'contextEstimated')
  const usage = useSessionField(session, 'usage')
  const model = useSessionField(session, 'model')
  const size = useSessionField(session, 'contextSize')
  const observed = useSessionField(session, 'observed')
  const { data: models } = useQuery({ enabled: canQuery1, queryKey: queryKeys.models, queryFn: () => api.list_models() })
  const { used, estimated, limit: own } = meterSource(observed, context, contextEstimated, size ?? undefined)
  if (used === undefined || memory === 'endless') return null
  const limit = own ?? models?.find((m) => m.id === model)?.context ?? 0
  const pct = limit > 0 ? Math.round((used / limit) * 100) : null
  const cache = usage && usage.prompt > 0 ? Math.round((usage.cached / usage.prompt) * 100) : null
  const [tone, warn] = BAND[band(pct)]
  const pill = (
    <NumberFlowGroup>
      <span
        className={cn('rounded-full border px-2.5 py-0.5 text-xs whitespace-nowrap tabular-nums', tone)}
        title={`${estimated ? 'Estimated context (≈) until the provider reports' : 'Context window used'}${compact ? '' : ' · last turn prefix cache hit rate'}${warn}`}
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
  return row ? <MenuRow icon={Gauge} label="Context" detail={pill} /> : pill
}
