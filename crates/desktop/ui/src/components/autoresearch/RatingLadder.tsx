import type { KindView } from '@/rpc/bindings/KindView'
import type { RatingRow } from '@/rpc/bindings/RatingRow'
import { shortSha } from './model'

/** 159 W3: the Bradley–Terry ladder of kept versions (θ ± 2 se, Elo-style label) and the judge–human agreement figure. Owner: W3. */
export function RatingLadder({ view }: { view: Extract<KindView, { kind: 'ratings' }> }) {
  const { ladder, agreement } = view
  const span = ladderSpan(ladder)
  return (
    <div data-kind={view.kind} className="space-y-3">
      <p className="text-xs text-fg-tertiary">
        {ladder.length === 0
          ? 'No judged pairs yet: the ladder appears after the first experiment.'
          : `${ladder.length} rung${ladder.length === 1 ? '' : 's'} · θ ± 2 se from every blind pair · base at the bottom unless a keep lost to it`}
        {agreement && (
          <span data-agreement className="ml-2 font-medium text-fg-secondary">
            you agreed with the judge on {agreement.agreed}/{agreement.asked} pair{agreement.asked === 1 ? '' : 's'}
          </span>
        )}
      </p>
      {ladder.length > 0 && (
        <ol className="space-y-1.5">
          {ladder.map((r, i) => (
            <li key={r.sha} data-rung={r.sha} className="grid grid-cols-[2.5rem_6rem_1fr_7rem] items-center gap-2 text-xs">
              <span className="text-fg-tertiary tabular-nums">#{i + 1}</span>
              <span className="truncate font-mono" title={r.sha}>
                {r.n === null ? 'base' : `exp ${r.n}`} <span className="text-fg-tertiary">{shortSha(r.sha)}</span>
              </span>
              <Bar theta={r.theta} se={r.se} span={span} />
              <span className="text-right tabular-nums" title={`θ ${r.theta.toFixed(2)} ± ${(2 * r.se).toFixed(2)} over ${r.games} pairs`}>
                Elo {Math.round(r.elo)} <span className="text-fg-tertiary">· {r.games}</span>
              </span>
            </li>
          ))}
        </ol>
      )}
    </div>
  )
}

/** The θ range every bar is drawn in: the widest interval plus a margin, symmetric around 0. */
function ladderSpan(rows: RatingRow[]): number {
  const edge = rows.reduce((m, r) => Math.max(m, Math.abs(r.theta) + 2 * r.se), 0)
  return Math.max(0.5, edge * 1.1)
}

/** θ as a dot on a line from −span to +span with its ± 2 se interval; 0 is the prior's centre. */
function Bar({ theta, se, span }: { theta: number; se: number; span: number }) {
  const pct = (x: number) => `${((x + span) / (2 * span)) * 100}%`
  return (
    <div className="relative h-3">
      <div className="absolute inset-y-1/2 right-0 left-0 h-px bg-border-subtle" />
      <div className="absolute inset-y-0 w-px bg-border" style={{ left: pct(0) }} />
      <div className="absolute inset-y-1/2 h-0.5 -translate-y-1/2 rounded bg-fg-tertiary/60" style={{ left: pct(theta - 2 * se), width: `calc(${pct(theta + 2 * se)} - ${pct(theta - 2 * se)})` }} />
      <div className="absolute top-1/2 h-2 w-2 -translate-x-1/2 -translate-y-1/2 rounded-full bg-foreground" style={{ left: pct(theta) }} />
    </div>
  )
}
