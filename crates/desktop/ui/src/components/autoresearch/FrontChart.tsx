import { useId, useState } from 'react'
import type { KindView } from '@/rpc/bindings/KindView'

type FrontView = Extract<KindView, { kind: 'front' }>

const W = 720
const H = 300
const PAD = { left: 56, right: 16, top: 12, bottom: 34 }

/**
 * 159 §6 "Study view": every experiment as a dot on the first two objectives (the base as #0), the
 * current front members outlined and joined by a staircase (lower-left is better on both axes). Plain
 * SVG; hover (or focus — every mark is a button) names the experiment and its two values.
 */
export function FrontChart({ view, onPick }: { view: FrontView; onPick?: (n: number) => void }) {
  const [xName, yName] = view.axes
  const id = useId()
  const [hover, setHover] = useState<{ x: number; y: number; text: string } | null>(null)
  const pts = view.points.flatMap((p) => {
    const x = p.scores[xName]
    const y = p.scores[yName]
    return x === undefined || y === undefined ? [] : [{ n: p.n, member: p.member, x, y }]
  })
  if (pts.length === 0) {
    return (
      <div className="rounded-md border border-dashed p-4 text-sm text-fg-tertiary" data-kind={view.kind}>
        No measured experiment yet — the front appears after the first sample.
      </div>
    )
  }
  const xs = pts.map((p) => p.x)
  const ys = pts.map((p) => p.y)
  const sx = scale(Math.min(...xs), Math.max(...xs), PAD.left, W - PAD.right)
  const sy = scale(Math.min(...ys), Math.max(...ys), H - PAD.bottom, PAD.top)
  const members = view.members
  const front = pts.filter((p) => members.includes(p.n)).sort((a, b) => a.x - b.x)
  // The staircase: the front members in x order; between two members the line runs along the
  // dominated corner (right, then down), so the area above/right of it is what the front beats.
  const stair = front
    .map((p, i) => {
      const x = sx(p.x).toFixed(1)
      const y = sy(p.y).toFixed(1)
      return i === 0 ? `M${x},${y}` : `H${x}V${y}`
    })
    .join(' ')
  return (
    <figure className="relative" data-kind={view.kind} data-members={members.length}>
      <svg viewBox={`0 0 ${W} ${H}`} width="100%" height={H} className="block max-w-full" role="img" aria-labelledby={`${id}-title`}>
        <title id={`${id}-title`}>{`${xName} against ${yName}: ${pts.length} experiments, ${members.length} on the front`}</title>
        <Axes xName={xName} yName={yName} xs={xs} ys={ys} sx={sx} sy={sy} />
        {front.length > 1 && <path d={stair} fill="none" stroke="var(--status-success-fg)" strokeWidth={1.8} strokeDasharray="4 3" vectorEffect="non-scaling-stroke" data-stair />}
        {pts.map((p) => {
          const x = sx(p.x)
          const y = sy(p.y)
          const text = `#${p.n}${p.member ? ' · on the front' : ''} · ${xName} ${fmt(p.x)} · ${yName} ${fmt(p.y)}`
          return (
            <circle
              key={p.n}
              cx={x}
              cy={y}
              r={p.member ? 6 : 4}
              fill={p.member ? 'var(--status-success-fg)' : p.n === 0 ? 'var(--fg-accent)' : 'var(--fg-tertiary)'}
              fillOpacity={p.member ? 1 : 0.7}
              stroke={p.member ? 'var(--bg-elevated)' : 'none'}
              strokeWidth={1.5}
              data-n={p.n}
              data-member={p.member ? 'true' : 'false'}
              tabIndex={0}
              role="button"
              aria-label={text}
              className="cursor-pointer outline-none focus-visible:stroke-fg-accent"
              onMouseEnter={() => setHover({ x, y, text })}
              onMouseLeave={() => setHover(null)}
              onFocus={() => setHover({ x, y, text })}
              onBlur={() => setHover(null)}
              onClick={() => onPick?.(p.n)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') onPick?.(p.n)
              }}
            />
          )
        })}
      </svg>
      {hover && (
        <div role="tooltip" className="pointer-events-none absolute z-10 max-w-[280px] -translate-x-1/2 -translate-y-full rounded-md border border-border bg-elevated px-2 py-1 text-xs shadow-md" style={{ left: `${(hover.x / W) * 100}%`, top: hover.y - 8 }}>
          {hover.text}
        </div>
      )}
      <figcaption className="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-xs text-fg-secondary">
        <span className="text-success">● front member · ┐ the front (lower-left is better on both)</span>
        <span>● base (#0)</span>
        <span className="text-fg-tertiary">● every other experiment</span>
      </figcaption>
    </figure>
  )
}

function scale(lo: number, hi: number, a: number, b: number) {
  const span = hi - lo || 1
  const pad = span * 0.06
  return (v: number) => a + ((v - (lo - pad)) / (span + 2 * pad)) * (b - a)
}

function fmt(v: number): string {
  if (v === 0) return '0'
  const digits = Math.max(0, 3 - Math.floor(Math.log10(Math.abs(v))) - 1)
  return v.toFixed(Math.min(digits, 4))
}

function Axes({ xName, yName, xs, ys, sx, sy }: { xName: string; yName: string; xs: number[]; ys: number[]; sx: (v: number) => number; sy: (v: number) => number }) {
  const ticks = (vals: number[]) => {
    const lo = Math.min(...vals)
    const hi = Math.max(...vals)
    return [...new Set([lo, (lo + hi) / 2, hi])]
  }
  return (
    <g fontSize={11} fill="currentColor" fillOpacity={0.55}>
      <line x1={PAD.left} x2={W - PAD.right} y1={H - PAD.bottom} y2={H - PAD.bottom} stroke="currentColor" strokeOpacity={0.25} />
      <line x1={PAD.left} x2={PAD.left} y1={PAD.top} y2={H - PAD.bottom} stroke="currentColor" strokeOpacity={0.25} />
      {ticks(xs).map((v) => (
        <text key={`x${v}`} x={sx(v)} y={H - PAD.bottom + 14} textAnchor="middle">
          {fmt(v)}
        </text>
      ))}
      {ticks(ys).map((v) => (
        <text key={`y${v}`} x={PAD.left - 6} y={sy(v) + 4} textAnchor="end">
          {fmt(v)}
        </text>
      ))}
      <text x={(PAD.left + W - PAD.right) / 2} y={H - 6} textAnchor="middle">
        {xName} →
      </text>
      <text x={12} y={(PAD.top + H - PAD.bottom) / 2} textAnchor="middle" transform={`rotate(-90 12 ${(PAD.top + H - PAD.bottom) / 2})`}>
        {yName} →
      </text>
    </g>
  )
}
