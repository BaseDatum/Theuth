import { useEffect, useId, useRef, useState } from 'react'
import { cn } from '@/lib/utils'
import { type Layout, OUTCOME_PAINT, layout } from './model'
import type { ExperimentRow } from "@/rpc/bindings/method.autoresearch.output"
import type { StudyDetail } from "@/rpc/bindings/method.autoresearch.output"

const H = 260

/**
 * §13.1 the chart: one dot per experiment coloured by outcome (× for a crash, hollow for interrupted), the incumbent as
 * a staircase, the baseline ± σ band, holdout diamonds on a right axis, drift markers. Plain SVG over `layout()`; hover
 * (or focus — every mark is a button) shows the verdict line in a tooltip, click opens the experiment.
 */
export function StudyChart({ detail, rows: runRows, onPick, className }: { detail: StudyDetail; rows?: readonly ExperimentRow[]; onPick: (n: number) => void; className?: string }) {
  const unit = detail.spec.metric.unit
  const ref = useRef<HTMLElement>(null)
  // Laid out in the container's CSS pixels (no `preserveAspectRatio="none"`): circles stay round, text stays legible.
  const [W, setW] = useState(900)
  useEffect(() => {
    const el = ref.current
    if (!el || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(([e]) => {
      if (e && e.contentRect.width > 0) setW(Math.round(e.contentRect.width))
    })
    ro.observe(el)
    return () => ro.disconnect()
  }, [])
  const l = layout(detail.series, { width: W, height: H, unit, lastN: detail.info.current ?? undefined, rows: runRows })
  const [hover, setHover] = useState<{ x: number; y: number; text: string } | null>(null)
  const id = useId()
  const rows = detail.series.experiments.length
  return (
    <figure ref={ref} className={cn('relative', className)}>
      <svg viewBox={`0 0 ${W} ${H}`} width={W} height={H} className="block max-w-full" role="img" aria-labelledby={`${id}-title`}>
        <title id={`${id}-title`}>
          {detail.spec.metric.name} per experiment: {rows} experiments, {detail.info.incumbent.kept} kept
        </title>
        <Axes l={l} unit={unit} />
        {l.band && <rect x={l.left} width={W - l.left - l.right} y={l.band.y} height={l.band.height} fill="var(--fg-accent)" fillOpacity={0.1} />}
        {l.stair && <path d={l.stair} fill="none" stroke="var(--status-success-fg)" strokeWidth={2.2} vectorEffect="non-scaling-stroke" />}
        {l.drift.map((d) => (
          <g key={`drift-${d.n}`}>
            <line x1={d.x} x2={d.x} y1={l.top} y2={H - l.bottom} stroke="var(--status-warning-fg)" strokeDasharray="3 3" strokeOpacity={0.7} vectorEffect="non-scaling-stroke" />
            <text x={d.x + 4} y={l.top + 10} fontSize={10} fill="var(--status-warning-fg)">
              drift
            </text>
          </g>
        ))}
        {l.marks.map((m) => (
          <Mark key={m.n} m={m} l={l} onPick={onPick} onHover={setHover} />
        ))}
        {l.holdout.map((h) => (
          <rect
            key={`holdout-${h.n}`}
            x={h.x - 4.5}
            y={h.y - 4.5}
            width={9}
            height={9}
            transform={`rotate(45 ${h.x} ${h.y})`}
            fill="var(--bg-elevated)"
            stroke="var(--fg-accent)"
            strokeWidth={1.6}
            onMouseEnter={() => setHover({ x: h.x, y: h.y, text: `holdout after #${h.n}: ${h.metric.toFixed(1)} ${unit}` })}
            onMouseLeave={() => setHover(null)}
          >
            <title>
              holdout after #{h.n}: {h.metric.toFixed(1)} {unit}
            </title>
          </rect>
        ))}
      </svg>
      {hover && (
        <div
          role="tooltip"
          className="pointer-events-none absolute z-10 max-w-[280px] -translate-x-1/2 -translate-y-full rounded-md border border-border bg-elevated px-2 py-1 text-xs shadow-md"
          style={{ left: hover.x, top: hover.y - 8 }}
        >
          {hover.text}
        </div>
      )}
      <figcaption className="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-xs text-fg-secondary">
        <Key paint={OUTCOME_PAINT.kept}>kept</Key>
        <Key paint={OUTCOME_PAINT.no_gain}>no gain / worse</Key>
        <Key paint={OUTCOME_PAINT.failed}>checks failed · × crash</Key>
        <Key paint={OUTCOME_PAINT.reverted}>reverted by review</Key>
        <Key paint="transparent" ring="var(--fg-tertiary)">
          interrupted
        </Key>
        {l.holdout.length > 0 && (
          <Key paint="transparent" ring="var(--fg-accent)" square>
            holdout (right axis)
          </Key>
        )}
        {l.band && <span>shaded: baseline ± σ</span>}
        <span className="text-success">— incumbent</span>
      </figcaption>
    </figure>
  )
}

function Axes({ l, unit }: { l: Layout; unit: string }) {
  return (
    <g fontSize={11} fill="currentColor" fillOpacity={0.55}>
      {l.ticksY.map((v) => (
        <g key={`y${v}`}>
          <line x1={l.left} x2={l.width - l.right} y1={l.y(v)} y2={l.y(v)} stroke="currentColor" strokeOpacity={0.08} vectorEffect="non-scaling-stroke" />
          <text x={l.left - 6} y={l.y(v) + 4} textAnchor="end">
            {Number.isInteger(v) ? v : v.toFixed(1)}
          </text>
        </g>
      ))}
      {l.ticksX.map((n) => (
        <text key={`x${n}`} x={l.x(n)} y={H - 8} textAnchor="middle">
          {n}
        </text>
      ))}
      <text x={l.left - 6} y={l.top - 3} textAnchor="end" fontSize={10}>
        {unit}
      </text>
      {l.yh &&
        l.ticksH.map((v) => (
          <text key={`h${v}`} x={l.width - l.right + 6} y={(l.yh as (v: number) => number)(v) + 4} fill="var(--fg-accent)" fillOpacity={0.8}>
            {Number.isInteger(v) ? v : v.toFixed(1)}
          </text>
        ))}
      {l.yh && (
        <text x={l.width - l.right + 6} y={l.top - 3} fontSize={10} fill="var(--fg-accent)" fillOpacity={0.8}>
          holdout
        </text>
      )}
    </g>
  )
}

/** One experiment: a circle (filled by outcome; hollow when interrupted), a × for a crash pinned to the top. Focusable and clickable. */
function Mark({ m, l, onPick, onHover }: { m: Layout['marks'][number]; l: Layout; onPick: (n: number) => void; onHover: (h: { x: number; y: number; text: string } | null) => void }) {
  const paint = OUTCOME_PAINT[m.outcome]
  const y = m.y ?? l.top + 6
  const common = {
    tabIndex: 0,
    role: 'button' as const,
    'aria-label': m.label,
    className: 'cursor-pointer outline-none focus-visible:stroke-[var(--ring)] focus-visible:stroke-2',
    onClick: () => onPick(m.n),
    onKeyDown: (e: { key: string; preventDefault(): void }) => {
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault()
        onPick(m.n)
      }
    },
    onMouseEnter: () => onHover({ x: m.x, y, text: m.label }),
    onFocus: () => onHover({ x: m.x, y, text: m.label }),
    onMouseLeave: () => onHover(null),
    onBlur: () => onHover(null),
  }
  if (m.y === null) {
    return (
      <g {...common} data-outcome={m.outcome} data-n={m.n}>
        <circle cx={m.x} cy={y} r={7} fill="transparent" />
        <path d={`M${m.x - 4},${y - 4} L${m.x + 4},${y + 4} M${m.x + 4},${y - 4} L${m.x - 4},${y + 4}`} stroke={paint} strokeWidth={2.2} vectorEffect="non-scaling-stroke" />
      </g>
    )
  }
  const hollow = m.outcome === 'lost' || m.outcome === 'open'
  return (
    <circle {...common} data-outcome={m.outcome} data-n={m.n} cx={m.x} cy={m.y} r={m.outcome === 'kept' ? 5 : 4} fill={hollow ? 'var(--bg-elevated)' : paint} stroke={hollow ? paint : 'none'} strokeWidth={1.6} />
  )
}

function Key({ paint, ring, square, children }: { paint: string; ring?: string; square?: boolean; children: string }) {
  return (
    <span className="inline-flex items-center gap-1.5">
      <span className={cn('inline-block size-2.5', square ? 'rotate-45 rounded-[1px]' : 'rounded-full')} style={{ background: paint, boxShadow: ring ? `inset 0 0 0 1.5px ${ring}` : undefined }} aria-hidden />
      {children}
    </span>
  )
}
