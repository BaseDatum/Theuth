// 123 §5.2(c) (W3): `Treemap` — squarified cells (Bruls/Huizing/van Wijk), groups as headed rectangles laid out by
// their totals. `TreemapSvg` is the pure drawing over `treemapLayout()` (snapshot-tested); `TreemapView` wraps it in
// `DiagramFrame` with the theme tokens. Labels appear only in cells ≥ 72 × 36 px; the tooltip carries the rest.
import { useId, useMemo, useRef } from 'react'
import type { z } from 'zod'
import { describe, svgA11y, unitLabel } from '@/lib/diagram/a11y'
import { fnv } from '@/lib/diagram/elk'
import { truncate } from '@/lib/diagram/label'
import { DiagramTooltip } from '@/lib/diagram/tooltip'
import type { DiagramHostProps, UnitState } from '@/lib/diagram/types'
import { useUnits } from '@/lib/diagram/units'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { useThemeTokens } from '@/lib/snapshot-mode'
import { DiagramFrame } from '../../DiagramFrame'
import type { treemapProps } from '../catalog'

export type TreemapData = z.infer<typeof treemapProps>

const M = 12
const HEADER = 20
const FONT = 13
const MONO = 'ui-monospace, monospace'
const LABEL_MIN_W = 72
const LABEL_MIN_H = 36

export interface Rect {
  x: number
  y: number
  w: number
  h: number
}

/**
 * Squarified treemap: rows of cells laid along the shorter side of the remaining box while the worst aspect ratio
 * keeps improving. Output index = input index (sorted internally by value, mapped back); non-positive values get an
 * empty rect at the box origin.
 */
export function squarify(values: number[], x: number, y: number, w: number, h: number): Rect[] {
  const out: Rect[] = values.map(() => ({ x, y, w: 0, h: 0 }))
  const total = values.reduce((s, v) => s + (v > 0 ? v : 0), 0)
  if (total <= 0 || w <= 0 || h <= 0) return out
  const scale = (w * h) / total
  const order = values.map((_, i) => i).filter((i) => (values[i] ?? 0) > 0)
  order.sort((a, b) => (values[b] ?? 0) - (values[a] ?? 0))
  const area = (i: number) => (values[i] ?? 0) * scale
  // worst aspect ratio of a row of `row` cells laid along a side of length `side`
  const worst = (row: number[], side: number) => {
    let s = 0
    let min = Number.POSITIVE_INFINITY
    let max = 0
    for (const i of row) {
      const a = area(i)
      s += a
      if (a < min) min = a
      if (a > max) max = a
    }
    const s2 = s * s
    const side2 = side * side
    return Math.max((side2 * max) / s2, s2 / (side2 * min))
  }
  let rx = x
  let ry = y
  let rw = w
  let rh = h
  let k = 0
  while (k < order.length) {
    const side = Math.min(rw, rh)
    const row: number[] = [order[k]!]
    k++
    while (k < order.length) {
      const next = [...row, order[k]!]
      if (worst(next, side) > worst(row, side)) break
      row.push(order[k]!)
      k++
    }
    const s = row.reduce((acc, i) => acc + area(i), 0)
    if (rw >= rh) {
      // column along the left edge
      const cw = s / rh
      let cy = ry
      for (const i of row) {
        const ch = area(i) / cw
        out[i] = { x: rx, y: cy, w: cw, h: ch }
        cy += ch
      }
      rx += cw
      rw -= cw
    } else {
      // row along the top edge
      const ch = s / rw
      let cx = rx
      for (const i of row) {
        const cw = area(i) / ch
        out[i] = { x: cx, y: ry, w: cw, h: ch }
        cx += cw
      }
      ry += ch
      rh -= ch
    }
  }
  return out
}

export interface TreemapLayout {
  /** Group frames in first-appearance order; `label` is undefined for the ungrouped remainder. */
  groups: (Rect & { label: string | undefined; header: boolean })[]
  /** Cell rects, index = item index; `group` indexes `groups`. */
  cells: (Rect & { group: number })[]
  total: number
  width: number
  height: number
}

/** Distinct group keys in first-appearance order (`''` = no group). */
function groupKeys(items: TreemapData['items']): string[] {
  const keys: string[] = []
  for (const it of items) {
    const k = it.group ?? ''
    if (!keys.includes(k)) keys.push(k)
  }
  return keys
}

/** Unit ids: the cell label, `label#<index>` when a label repeats. */
export function treemapIds(items: TreemapData['items']): string[] {
  const seen = new Set<string>()
  return items.map((it, i) => {
    const id = seen.has(it.label) ? `${it.label}#${i}` : it.label
    seen.add(it.label)
    return id
  })
}

export function treemapLayout(d: TreemapData, W = 560, top = 0): TreemapLayout {
  const mapH = Math.round((W * 0.6) / 4) * 4
  const keys = groupKeys(d.items)
  const headed = keys.some((k) => k !== '')
  const total = d.items.reduce((s, it) => s + (it.value > 0 ? it.value : 0), 0)
  const totals = keys.map((k) => d.items.reduce((s, it) => s + ((it.group ?? '') === k && it.value > 0 ? it.value : 0), 0))
  const frames = headed ? squarify(totals, M, top + M, W, mapH) : [{ x: M, y: top + M, w: W, h: mapH }]
  const groups = frames.map((f, g) => (headed ? { x: f.x + 2, y: f.y + 2, w: Math.max(0, f.w - 4), h: Math.max(0, f.h - 4), label: keys[g] || undefined, header: true } : { ...f, label: undefined, header: false }))
  const cells: TreemapLayout['cells'] = d.items.map(() => ({ x: M, y: top + M, w: 0, h: 0, group: 0 }))
  groups.forEach((g, gi) => {
    const idx = d.items.map((_, i) => i).filter((i) => (d.items[i]?.group ?? '') === keys[gi])
    const inner = g.header ? { x: g.x, y: g.y + HEADER, w: g.w, h: Math.max(0, g.h - HEADER) } : g
    const rects = squarify(
      idx.map((i) => d.items[i]?.value ?? 0),
      inner.x,
      inner.y,
      inner.w,
      inner.h,
    )
    idx.forEach((i, j) => {
      const r = rects[j]!
      // 2 px gutter: 1 px inset on each side
      cells[i] = { x: r.x + 1, y: r.y + 1, w: Math.max(0, r.w - 2), h: Math.max(0, r.h - 2), group: gi }
    })
  })
  return { groups, cells, total, width: W + 2 * M, height: top + mapH + 2 * M }
}

function tint(state: UnitState, t: DiagramTokens): { stroke: string; fill: string; fillOpacity: number; opacity: number; width: number } | null {
  switch (state) {
    case 'active':
      return { stroke: t.accent, fill: t.accent, fillOpacity: 0.1, opacity: 1, width: 2 }
    case 'done':
      return { stroke: t.success, fill: t.success, fillOpacity: 0.12, opacity: 1, width: 1.25 }
    case 'danger':
      return { stroke: t.danger, fill: t.danger, fillOpacity: 0.12, opacity: 1, width: 1.5 }
    default:
      return null
  }
}

/** The `<desc>`: the first line of the Rust lowering (`render.rs` Treemap arm). */
export function treemapDesc(d: TreemapData): string {
  return describe('Treemap', d.title, d.items.length, 'items')
}

export function TreemapSvg({ data: d, layout: l, tokens: t, markerId, highlight, onSelect }: { data: TreemapData; layout: TreemapLayout; tokens: DiagramTokens; markerId: string; highlight?: readonly string[]; onSelect?: (id: string) => void }) {
  const ids = useMemo(() => treemapIds(d.items), [d.items])
  const incident = useMemo(
    () => (id: string) => {
      const g = l.cells[ids.indexOf(id)]?.group
      return ids.filter((other, i) => other !== id && l.cells[i]?.group === g)
    },
    [ids, l.cells],
  )
  const u = useUnits({ ids, incident, highlight, onSelect })
  const a = svgA11y(markerId, !!onSelect)
  const ref = useRef<SVGSVGElement>(null)
  const hi = u.state.hover === null ? -1 : ids.indexOf(u.state.hover)
  const hover = d.items[hi]
  const share = hover && l.total > 0 ? `${((100 * hover.value) / l.total).toFixed(1)}%` : undefined
  return (
    <>
      <svg ref={ref} xmlns="http://www.w3.org/2000/svg" viewBox={`0 0 ${l.width} ${l.height}`} width={l.width} height={l.height} style={{ maxWidth: '100%', height: 'auto' }} {...a.svg} {...u.root}>
        <title id={a.titleId}>{d.title ?? 'Treemap'}</title>
        <desc id={a.descId}>{treemapDesc(d)}</desc>
        {d.title && (
          <text x={M} y={18} fontFamily={t.font} fontSize={FONT} fontWeight={600} fill={t.foreground}>
            {d.title}
          </text>
        )}
        {l.groups.map(
          (g, gi) =>
            g.header && (
              <g key={gi}>
                <rect x={g.x} y={g.y} width={g.w} height={g.h} rx={4} fill="none" stroke={t.border} strokeWidth={1} />
                {g.w >= 40 && (
                  <text x={g.x + 6} y={g.y + 14} fontFamily={t.font} fontSize={11} fontWeight={600} fill={t.mutedForeground}>
                    {truncate(g.label ?? 'other', Math.max(3, Math.floor((g.w - 12) / (11 * 0.6))))}
                  </text>
                )}
              </g>
            ),
        )}
        {d.items.map((it, i) => {
          const c = l.cells[i]
          const id = ids[i]
          if (!c || id === undefined) return null
          const state = u.paint(id, it.state)
          const p = tint(state, t)
          const hitW = Math.max(18, c.w)
          const hitH = Math.max(18, c.h)
          const showText = c.w >= LABEL_MIN_W && c.h >= LABEL_MIN_H
          return (
            <g key={id} {...u.unit(id, unitLabel(it.label, `${it.value}`, it.state))} aria-description={it.ref ?? undefined} opacity={state === 'muted' ? 0.5 : 1}>
              {(hitW > c.w || hitH > c.h) && <rect x={c.x + c.w / 2 - hitW / 2} y={c.y + c.h / 2 - hitH / 2} width={hitW} height={hitH} fill="transparent" />}
              <rect x={c.x} y={c.y} width={c.w} height={c.h} rx={4} fill={t.series[c.group % 6]} fillOpacity={0.25} />
              {p && <rect x={c.x} y={c.y} width={c.w} height={c.h} rx={4} fill={p.fill} fillOpacity={p.fillOpacity} stroke={p.stroke} strokeWidth={p.width} />}
              {showText && (
                <>
                  <text x={c.x + 8} y={c.y + 16} fontFamily={t.font} fontSize={FONT} fontWeight={600} fill={t.foreground}>
                    {truncate(it.label, Math.max(4, Math.floor((c.w - 16) / (FONT * 0.6))))}
                  </text>
                  <text x={c.x + 8} y={c.y + 30} fontFamily={MONO} fontSize={11} fill={t.mutedForeground}>
                    {it.value}
                  </text>
                </>
              )}
            </g>
          )
        })}
      </svg>
      <DiagramTooltip
        hover={u.state.hover}
        root={ref.current}
        title={hover?.label ?? ''}
        rows={[
          { label: 'value', value: hover ? `${hover.value}` : undefined },
          { label: 'share', value: share },
          { label: 'group', value: hover?.group },
          { label: 'state', value: hover?.state },
          { label: 'ref', value: hover?.ref },
        ]}
      />
    </>
  )
}

export function TreemapView({ props, highlight, onSelect }: DiagramHostProps<TreemapData>) {
  const id = useId().replace(/:/g, '')
  const hash = useMemo(() => fnv(JSON.stringify(props)), [props])
  const tokens = useThemeTokens()
  const layout = useMemo(() => treemapLayout(props, 560, props.title ? 28 : 0), [props])
  return (
    <DiagramFrame name={`treemap-${hash}`} label="treemap" kind="treemap-block">
      <TreemapSvg data={props} layout={layout} tokens={tokens} markerId={`treemap-${id}`} highlight={highlight} onSelect={onSelect} />
    </DiagramFrame>
  )
}
