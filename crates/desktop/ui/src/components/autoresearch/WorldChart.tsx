import { useId } from 'react'
import type { ArmWindow } from '@/rpc/bindings/ArmWindow'
import type { KindView } from '@/rpc/bindings/KindView'

type WorldView = Extract<KindView, { kind: 'world' }>

const W = 720
const H = 200
const PAD = { left: 56, right: 16, top: 14, bottom: 26 }
const ARM_PAINT: Record<string, string> = { control: 'var(--fg-tertiary)', candidate: 'var(--fg-accent)' }

const fmt = (v: number | null | undefined, digits = 4) => (v == null || !Number.isFinite(v) ? '—' : v.toFixed(digits))
const clock = (iso: string) => {
  const t = new Date(iso)
  return Number.isNaN(t.getTime()) ? iso : t.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })
}

/** 95 % interval of a window's mean: ± 1.96 · √(var / units). */
export function interval(w: ArmWindow): number {
  if (w.var == null || w.units <= 0) return 0
  return 1.96 * Math.sqrt(Math.max(w.var, 0) / w.units)
}

/** The windows of the experiment the chart shows: the latest launched one. */
export function latestLaunch(windows: readonly ArmWindow[]): { n: number | null; rows: ArmWindow[] } {
  if (windows.length === 0) return { n: null, rows: [] }
  const n = Math.max(...windows.map((w) => w.n))
  return { n, rows: windows.filter((w) => w.n === n).sort((a, b) => a.win - b.win || a.arm.localeCompare(b.arm)) }
}

/**
 * 159 W6 `KindView::World`: per window both arms' means with their 95 % intervals, Λ after each window on a log
 * axis against the 1/α line, the launch row, and a units table (exposure is the two arms' share of the total).
 */
export function WorldChart({ view, alpha = 0.01 }: { view: WorldView; alpha?: number }) {
  const id = useId()
  const { n, rows } = latestLaunch(view.windows)
  const wins = Array.from(new Set(rows.map((r) => r.win))).sort((a, b) => a - b)
  const launch = view.launch
  if (n == null) {
    return (
      <div className="space-y-2 text-sm" data-kind={view.kind}>
        <LaunchLine launch={launch} />
        <p className="text-fg-tertiary">No window measured yet{launch?.state === 'requested' ? ' — launch waits for your press.' : '.'}</p>
      </div>
    )
  }
  // Means chart.
  const lo = Math.min(...rows.map((r) => (r.mean ?? 0) - interval(r)))
  const hi = Math.max(...rows.map((r) => (r.mean ?? 0) + interval(r)))
  const span = hi - lo || 1
  const x = (win: number) => PAD.left + ((wins.indexOf(win) + 0.5) / wins.length) * (W - PAD.left - PAD.right)
  const y = (v: number) => PAD.top + (1 - (v - lo) / span) * (H - PAD.top - PAD.bottom)
  // Λ chart (log10), the 1/α line.
  const threshold = 1 / alpha
  const lam = view.lambda.map((v) => Math.max(v, 1e-3))
  const lmax = Math.max(threshold * 3, ...lam)
  const lmin = Math.min(1e-1, ...lam)
  const ly = (v: number) => PAD.top + (1 - (Math.log10(v) - Math.log10(lmin)) / (Math.log10(lmax) - Math.log10(lmin) || 1)) * (H - PAD.top - PAD.bottom)
  const lx = (i: number) => PAD.left + ((i + 0.5) / Math.max(lam.length, 1)) * (W - PAD.left - PAD.right)
  const lastLambda = view.lambda.at(-1)
  return (
    <div className="space-y-3 text-sm" data-kind={view.kind} data-n={n}>
      <LaunchLine launch={launch} />
      <figure>
        <svg viewBox={`0 0 ${W} ${H}`} className="block h-auto w-full" role="img" aria-labelledby={`${id}-arms`}>
          <title id={`${id}-arms`}>
            Launch #{n}: both arms&apos; means with 95 % intervals over {wins.length} window{wins.length === 1 ? '' : 's'}
          </title>
          <line x1={PAD.left} x2={PAD.left} y1={PAD.top} y2={H - PAD.bottom} stroke="currentColor" strokeOpacity={0.3} />
          <line x1={PAD.left} x2={W - PAD.right} y1={H - PAD.bottom} y2={H - PAD.bottom} stroke="currentColor" strokeOpacity={0.3} />
          {[lo, (lo + hi) / 2, hi].map((v) => (
            <text key={`y-${v}`} x={PAD.left - 6} y={y(v) + 3} fontSize={10} textAnchor="end" fill="currentColor" fillOpacity={0.7}>
              {fmt(v)}
            </text>
          ))}
          {wins.map((win) => (
            <text key={`x-${win}`} x={x(win)} y={H - PAD.bottom + 14} fontSize={10} textAnchor="middle" fill="currentColor" fillOpacity={0.7}>
              w{win}
            </text>
          ))}
          {rows.map((r) => {
            const dx = r.arm === 'control' ? -6 : 6
            const cx = x(r.win) + dx
            const m = r.mean ?? 0
            const ci = interval(r)
            const paint = ARM_PAINT[r.arm] ?? 'currentColor'
            return (
              <g key={`${r.arm}-${r.win}`} data-arm={r.arm} data-win={r.win}>
                <line x1={cx} x2={cx} y1={y(m - ci)} y2={y(m + ci)} stroke={paint} strokeWidth={1.5} />
                <circle cx={cx} cy={y(m)} r={3.5} fill={paint}>
                  <title>
                    {r.arm} · window {r.win}: {fmt(r.mean)} ± {fmt(ci)} on {r.units} units ({clock(r.start_at)} → {clock(r.end_at)})
                  </title>
                </circle>
              </g>
            )
          })}
        </svg>
        <figcaption className="flex gap-4 text-xs text-fg-tertiary">
          <span>
            <span className="inline-block h-2 w-2 rounded-full align-middle" style={{ background: ARM_PAINT.control }} /> control (incumbent)
          </span>
          <span>
            <span className="inline-block h-2 w-2 rounded-full align-middle" style={{ background: ARM_PAINT.candidate }} /> candidate
          </span>
        </figcaption>
      </figure>
      {lam.length > 0 && (
        <figure>
          <svg viewBox={`0 0 ${W} ${H}`} className="block h-auto w-full" role="img" aria-labelledby={`${id}-lambda`}>
            <title id={`${id}-lambda`}>
              Λ after each window against 1/α = {threshold}; now {fmt(lastLambda, 2)}
            </title>
            <line x1={PAD.left} x2={PAD.left} y1={PAD.top} y2={H - PAD.bottom} stroke="currentColor" strokeOpacity={0.3} />
            <line x1={PAD.left} x2={W - PAD.right} y1={H - PAD.bottom} y2={H - PAD.bottom} stroke="currentColor" strokeOpacity={0.3} />
            <line x1={PAD.left} x2={W - PAD.right} y1={ly(threshold)} y2={ly(threshold)} stroke="var(--status-success-fg)" strokeDasharray="4 3" data-threshold={threshold} />
            <text x={W - PAD.right} y={ly(threshold) - 4} fontSize={10} textAnchor="end" fill="var(--status-success-fg)">
              keep: Λ ≥ 1/α = {threshold}
            </text>
            <line x1={PAD.left} x2={W - PAD.right} y1={ly(1)} y2={ly(1)} stroke="currentColor" strokeOpacity={0.2} />
            <text x={PAD.left - 6} y={ly(1) + 3} fontSize={10} textAnchor="end" fill="currentColor" fillOpacity={0.7}>
              1
            </text>
            <path
              d={lam.map((v, i) => `${i === 0 ? 'M' : 'L'}${lx(i).toFixed(1)},${ly(v).toFixed(1)}`).join(' ')}
              fill="none"
              stroke="var(--fg-accent)"
              strokeWidth={2}
            />
            {lam.map((v, i) => (
              <circle key={`l-${i}`} cx={lx(i)} cy={ly(v)} r={3} fill="var(--fg-accent)" data-lambda={view.lambda[i]}>
                <title>
                  window {i + 1}: Λ = {fmt(view.lambda[i], 2)} (p ≤ {fmt(Math.min(1, 1 / (view.lambda[i] || 1)), 3)})
                </title>
              </circle>
            ))}
          </svg>
          <figcaption className="text-xs text-fg-tertiary">Λ per window (log axis), the always-valid test; the dashed line is the keep threshold.</figcaption>
        </figure>
      )}
      <table className="w-full text-xs">
        <thead className="text-fg-tertiary">
          <tr>
            <th className="text-left font-normal">window</th>
            <th className="text-left font-normal">closes</th>
            <th className="text-right font-normal">control units</th>
            <th className="text-right font-normal">candidate units</th>
            <th className="text-right font-normal">control mean</th>
            <th className="text-right font-normal">candidate mean</th>
            <th className="text-right font-normal">Λ</th>
          </tr>
        </thead>
        <tbody>
          {wins.map((win, i) => {
            const c = rows.find((r) => r.win === win && r.arm === 'control')
            const k = rows.find((r) => r.win === win && r.arm === 'candidate')
            return (
              <tr key={win} data-win={win}>
                <td>{win}</td>
                <td>{clock((c ?? k)?.end_at ?? '')}</td>
                <td className="text-right tabular-nums">{c?.units ?? '—'}</td>
                <td className="text-right tabular-nums">{k?.units ?? '—'}</td>
                <td className="text-right tabular-nums">{fmt(c?.mean)}</td>
                <td className="text-right tabular-nums">{fmt(k?.mean)}</td>
                <td className="text-right tabular-nums">{fmt(view.lambda[i], 2)}</td>
              </tr>
            )
          })}
        </tbody>
      </table>
    </div>
  )
}

function LaunchLine({ launch }: { launch: WorldView['launch'] }) {
  if (!launch) return null
  const when = launch.decided_at ? ` · ${launch.state} ${clock(launch.decided_at)}${launch.by ? ` by ${launch.by}` : ''}` : ` · requested ${clock(launch.requested_at)}`
  return (
    <p className="text-fg-secondary" data-launch={launch.state}>
      Launch #{launch.n}: {(launch.exposure * 100).toFixed(0)} % of traffic in two arms ({(launch.exposure * 50).toFixed(0)} % each)
      {when}
    </p>
  )
}
