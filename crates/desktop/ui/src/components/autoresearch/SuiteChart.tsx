import { useId } from 'react'
import type { KindView } from '@/rpc/bindings/KindView'
import type { SuiteRow } from '@/rpc/bindings/SuiteRow'

const H = 220
const TOP = 16
const BOTTOM = 44
const LEFT = 40
const GROUP = 64
const BAR = 20
const GAP = 28

type Suite = Extract<KindView, { kind: 'suite' }>

/**
 * 159 W7 `KindView::Suite`: per fixture, two bars — the incumbent mechanism's normalised holdout gain and the newest
 * candidate's — training fixtures left, held-out fixtures right of a divider (their numbers come from the outer
 * holdout and never from the brief). 1.0 is the fixture's reference solution; 0 is the base; a bar below the axis is a
 * regression. Plain SVG; each bar carries `data-fixture`, `data-held-out` and a `<title>` tooltip.
 */
export function SuiteChart({ view }: { view: Suite }) {
  const id = useId()
  const training = view.fixtures.filter((f) => !f.held_out)
  const held = view.fixtures.filter((f) => f.held_out)
  const rows: SuiteRow[] = [...training, ...held]
  if (rows.length === 0) {
    return (
      <div className="rounded-md border border-dashed p-4 text-sm text-fg-tertiary" data-kind={view.kind}>
        No fixtures in the suite yet.
      </div>
    )
  }
  const values = rows.flatMap((r) => [r.incumbent, r.candidate]).filter((v): v is number => v !== null && Number.isFinite(v))
  const hi = Math.max(1, ...values)
  const lo = Math.min(0, ...values)
  const W = LEFT + rows.length * GROUP + (held.length > 0 ? GAP : 0) + 16
  const plotH = H - TOP - BOTTOM
  const y = (v: number) => TOP + ((hi - v) / (hi - lo)) * plotH
  const y0 = y(0)
  const x0 = (i: number) => LEFT + i * GROUP + (i >= training.length ? GAP : 0)
  const fmt = (v: number | null) => (v === null ? '—' : v.toFixed(2))
  return (
    <figure className="relative" data-kind={view.kind}>
      <svg viewBox={`0 0 ${W} ${H}`} width={W} height={H} className="block max-w-full" role="img" aria-labelledby={`${id}-title`}>
        <title id={`${id}-title`}>
          Suite: {training.length} training and {held.length} held-out fixtures, incumbent against candidate mechanism
        </title>
        {[1, 0.5, 0].filter((t) => t <= hi && t >= lo).map((t) => (
          <g key={t}>
            <line x1={LEFT} x2={W - 8} y1={y(t)} y2={y(t)} stroke="currentColor" strokeOpacity={t === 0 ? 0.35 : 0.08} vectorEffect="non-scaling-stroke" />
            <text x={LEFT - 6} y={y(t) + 3} fontSize={10} textAnchor="end" fill="currentColor" fillOpacity={0.55}>
              {t === 1 ? 'ref' : t === 0 ? 'base' : '0.5'}
            </text>
          </g>
        ))}
        {held.length > 0 && (
          <g>
            <line x1={x0(training.length) - GAP / 2} x2={x0(training.length) - GAP / 2} y1={TOP} y2={H - BOTTOM + 8} stroke="currentColor" strokeDasharray="3 3" strokeOpacity={0.4} vectorEffect="non-scaling-stroke" />
            <text x={x0(training.length) - GAP / 2 + 4} y={TOP + 10} fontSize={10} fill="currentColor" fillOpacity={0.55}>
              held out
            </text>
          </g>
        )}
        {rows.map((r, i) => {
          const gx = x0(i)
          const bars: { key: 'incumbent' | 'candidate'; v: number | null; dx: number; fill: string }[] = [
            { key: 'incumbent', v: r.incumbent, dx: 4, fill: 'var(--fg-accent)' },
            { key: 'candidate', v: r.candidate, dx: 8 + BAR, fill: 'var(--status-success-fg)' },
          ]
          return (
            <g key={r.fixture} data-fixture={r.fixture} data-held-out={r.held_out ? 'true' : 'false'}>
              {bars.map((b) =>
                b.v === null ? null : (
                  <rect
                    key={b.key}
                    x={gx + b.dx}
                    y={Math.min(y0, y(b.v))}
                    width={BAR}
                    height={Math.max(1, Math.abs(y0 - y(b.v)))}
                    fill={b.fill}
                    fillOpacity={r.held_out ? 0.55 : 0.9}
                    data-role={b.key}
                    data-value={b.v.toFixed(3)}
                  >
                    <title>
                      {r.fixture} · {b.key} {fmt(b.v)}
                      {r.held_out ? ' (held out)' : ''}
                    </title>
                  </rect>
                ),
              )}
              <text x={gx + GROUP / 2} y={H - BOTTOM + 14} fontSize={10} textAnchor="middle" fill="currentColor" fillOpacity={0.75}>
                {r.fixture.replace(/^toy-/, '')}
              </text>
              <text x={gx + GROUP / 2} y={H - BOTTOM + 27} fontSize={9} textAnchor="middle" fill="currentColor" fillOpacity={0.5}>
                {fmt(r.incumbent)} → {fmt(r.candidate)}
              </text>
            </g>
          )
        })}
      </svg>
      <figcaption className="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-xs text-fg-secondary">
        <span>
          <span className="mr-1 inline-block size-2.5 rounded-sm align-middle" style={{ background: 'var(--fg-accent)' }} aria-hidden />
          incumbent mechanism
        </span>
        <span>
          <span className="mr-1 inline-block size-2.5 rounded-sm align-middle" style={{ background: 'var(--status-success-fg)' }} aria-hidden />
          newest candidate
        </span>
        <span>normalised holdout gain · 0 = the fixture as shipped · ref = its reference solution</span>
      </figcaption>
    </figure>
  )
}
