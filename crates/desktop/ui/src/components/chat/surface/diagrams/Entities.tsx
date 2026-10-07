// Design 123 §4.2, §5.2(a), §5.4 #4 (W1): `Entities` — ELK layered with ports: a relation that names a field
// (`from_field` / `to_field`) attaches at that field's row through a `FIXED_POS` port on the box side (east for the
// source, west for the target in LR; south/north in TB); a relation without one attaches to the box itself and ELK
// keeps attach points ≥ 12 px apart (`spacing.portPort`). Boxes have compartments: name (+ kind), fields (type and
// key/nullable columns in mono under `detail: schema`), methods under `detail: class`; `+n more` past 12 fields.
// Crow's-foot markers for the cardinalities, UML heads for the class relations.
import type { ElkNode, ElkPort } from 'elkjs/lib/elk-api.js'
import { type ReactNode, useEffect, useMemo, useRef } from 'react'
import type { z } from 'zod'
import { describe, svgA11y, unitLabel } from '@/lib/diagram/a11y'
import { ELK_OPTIONS, fnv, labelSize, norm, PAD_X, textWidth } from '@/lib/diagram/elk'
import { MaskedLabel } from '@/lib/diagram/label'
import { sectionsPath } from '@/lib/diagram/path'
import type { DiagramHostProps, UnitState } from '@/lib/diagram/types'
import type { Units } from '@/lib/diagram/units'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { EDGE_OPTIONS, flattenElk, FONT_PX, type Layout, useDiagramTokens, useElkLayout } from '@/lib/graph-layout'
import { edgeLabelAt, ElkFrame, MARGIN, MONO, edgeUnit, paint, type Tip, TITLE_H, useGraphUnits } from '../Graph'
import type { entitiesProps } from '../catalog'

export type EntitiesData = z.infer<typeof entitiesProps>
type Entity = EntitiesData['entities'][number]
type Relation = EntitiesData['relations'][number]
type Detail = NonNullable<EntitiesData['detail']>

/** Fields shown before `+n more` (the Rust `SHOWN_FIELDS`). */
export const SHOWN_FIELDS = 12
export const HEAD_H = 26
export const ROW_H = 18
const MONO_PX = 11
const SEP = 1

/** Entities present; relations whose ends exist (a field no longer present falls back to the box side). */
export function dropDangling(p: EntitiesData): EntitiesData {
  const ids: Record<string, true> = {}
  for (const e of p.entities) ids[e.id] = true
  return { ...p, relations: p.relations.filter((r) => ids[r.from] && ids[r.to]) }
}

export function structuralHash(p: EntitiesData): string {
  return fnv(
    [
      `en:${p.detail ?? 'model'}:${norm(p.title)}`,
      ...p.entities.map((e) => `e\u0001${e.id}\u0001${e.kind ?? ''}\u0001${norm(e.label)}\u0001${e.state ?? ''}\u0001${e.fields.map((f) => `${f.name}:${f.type ?? ''}:${f.key ?? ''}:${f.nullable ? 1 : 0}`).join(',')}\u0001${(e.methods ?? []).map((m) => `${m.name}${m.sig ?? ''}`).join(',')}`),
      ...p.relations.map((r) => `r\u0001${r.from}\u0001${r.to}\u0001${r.kind}\u0001${norm(r.label)}\u0001${r.from_field ?? ''}\u0001${r.to_field ?? ''}`),
    ].join('\u0002'),
  )
}

/** The text columns of one field row under `detail`: `[name, type, key]`. */
export function fieldColumns(f: Entity['fields'][number], detail: Detail): [string, string, string] {
  const key = [f.key ?? '', f.nullable ? 'null' : ''].filter(Boolean).join(' ')
  if (detail === 'model') return [f.name, '', '']
  if (detail === 'class') return [f.type ? `${f.name}: ${f.type}` : f.name, '', '']
  return [f.name, f.type ?? '', key]
}

export interface EntityBox {
  w: number
  h: number
  /** Rows shown (fields, capped) — the `+n more` row is extra. */
  shown: number
  more: number
  methods: string[]
  /** Width of the name column and the type column (schema), for the column layout. */
  cols: [number, number, number]
}

/** The compartments' size for `e` under `detail` (4 px grid). */
export function entityBox(e: Entity, detail: Detail): EntityBox {
  const shown = Math.min(e.fields.length, SHOWN_FIELDS)
  const more = e.fields.length - shown
  const methods = detail === 'class' ? (e.methods ?? []).map((m) => `${m.name}${m.sig ?? '()'}`) : []
  const cols: [number, number, number] = [0, 0, 0]
  for (const f of e.fields.slice(0, shown)) {
    const c = fieldColumns(f, detail)
    for (let i = 0; i < 3; i++) cols[i] = Math.max(cols[i]!, c[i] ? textWidth(c[i]!, MONO_PX) + (i < 2 ? 12 : 0) : 0)
  }
  const kind = e.kind && e.kind !== 'entity' ? textWidth(e.kind, MONO_PX) + 12 : 0
  const headW = textWidth(e.label, FONT_PX) + kind
  const methodsW = Math.max(0, ...methods.map((m) => textWidth(m, MONO_PX)))
  const moreW = more ? textWidth(`+${more} more`, MONO_PX) : 0
  const w = Math.max(96, Math.ceil((Math.max(headW, cols[0] + cols[1] + cols[2], methodsW, moreW) + 2 * PAD_X) / 4) * 4)
  const rows = shown + (more ? 1 : 0)
  const h = HEAD_H + (rows ? rows * ROW_H + 8 : 0) + (methods.length ? methods.length * ROW_H + 8 : 0)
  return { w, h: Math.ceil(h / 4) * 4, shown, more, methods, cols }
}

/** The y (box-relative) of field `name`'s row centre, or `null` when it is not shown. */
export function fieldY(e: Entity, name: string): number | null {
  const i = e.fields.findIndex((f) => f.name === name)
  return i < 0 || i >= SHOWN_FIELDS ? null : HEAD_H + 4 + i * ROW_H + ROW_H / 2
}

export const portId = (entity: string, field: string, side: 'from' | 'to') => `p:${entity}:${field}:${side}`

/**
 * Entities as nodes with `FIXED_POS` ports at the rows their relations name; a relation uses its field port when it
 * has one and the box otherwise.
 */
export function toElk(p: EntitiesData): ElkNode {
  const detail = p.detail ?? 'model'
  const by: Record<string, Entity> = {}
  for (const e of p.entities) by[e.id] = e
  const boxes: Record<string, EntityBox> = {}
  for (const e of p.entities) boxes[e.id] = entityBox(e, detail)
  const ports: Record<string, ElkPort[]> = {}
  const endpoint = (entity: string, field: string | null | undefined, side: 'from' | 'to'): string => {
    const e = by[entity]
    const box = boxes[entity]
    const y = e && field ? fieldY(e, field) : null
    if (!e || !box || y === null) return `n:${entity}`
    const id = portId(entity, field!, side)
    const list = (ports[entity] ??= [])
    if (!list.some((x) => x.id === id)) list.push({ id, x: side === 'from' ? box.w : 0, y, width: 0, height: 0 })
    return id
  }
  const edges = p.relations.map((r, i) => {
    const src = endpoint(r.from, r.from_field, 'from')
    const dst = endpoint(r.to, r.to_field, 'to')
    const text = norm(r.label)
    const l = labelSize(text)
    return { id: `e:${i}`, sources: [src], targets: [dst], labels: l ? [{ id: `l:${i}`, text, width: l.w, height: l.h }] : [] }
  })
  return {
    id: 'root',
    layoutOptions: { ...ELK_OPTIONS, ...EDGE_OPTIONS, 'elk.direction': 'RIGHT', 'elk.layered.spacing.nodeNodeBetweenLayers': '64' },
    children: p.entities.map((e) => {
      const b = boxes[e.id]!
      const ps = ports[e.id]
      return { id: `n:${e.id}`, width: b.w, height: b.h, ...(ps ? { ports: ps, layoutOptions: { 'org.eclipse.elk.portConstraints': 'FIXED_POS' } } : { layoutOptions: { 'org.eclipse.elk.portConstraints': 'FIXED_SIDE' } }) }
    }),
    edges,
  }
}

/** The marker at each end of a relation: `[start, end]` marker names, and whether the stroke is dashed. */
export function relationMarks(kind: Relation['kind']): { start: string | null; end: string | null; dashed: boolean } {
  switch (kind) {
    case 'one-one':
      return { start: 'bar', end: 'bar', dashed: false }
    case 'one-many':
      return { start: 'bar', end: 'crow', dashed: false }
    case 'many-many':
      return { start: 'crow', end: 'crow', dashed: false }
    case 'inherits':
      return { start: null, end: 'tri', dashed: false }
    case 'implements':
      return { start: null, end: 'tri', dashed: true }
    case 'composes':
      return { start: 'diaf', end: null, dashed: false }
    case 'aggregates':
      return { start: 'dia', end: null, dashed: false }
    case 'depends':
      return { start: null, end: 'open', dashed: true }
    case 'references':
      return { start: null, end: 'open', dashed: false }
  }
}

/** Crow's-foot and UML markers, `${id}-${name}`; all `auto-start-reverse` so one definition serves both ends. */
export function RelationMarkers({ id, tokens: t }: { id: string; tokens: DiagramTokens }) {
  const m = (name: string, body: ReactNode, w = 12) => (
    <marker key={name} id={`${id}-${name}`} viewBox="0 0 12 12" refX={w} refY="6" markerWidth={w} markerHeight="12" markerUnits="userSpaceOnUse" orient="auto-start-reverse">
      {body}
    </marker>
  )
  return (
    <>
      {m('bar', <path d="M9 1 V11 M0 6 H12" fill="none" stroke={t.foreground} strokeWidth={1.25} />)}
      {m('crow', <path d="M0 6 H12 M12 1 L4 6 L12 11" fill="none" stroke={t.foreground} strokeWidth={1.25} />)}
      {m('tri', <path d="M0 1 L12 6 L0 11 Z" fill={t.card} stroke={t.foreground} strokeWidth={1.25} />)}
      {m('dia', <path d="M0 6 L6 1 L12 6 L6 11 Z" fill={t.card} stroke={t.foreground} strokeWidth={1.25} />)}
      {m('diaf', <path d="M0 6 L6 1 L12 6 L6 11 Z" fill={t.foreground} />)}
      {m('open', <path d="M2 1 L12 6 L2 11" fill="none" stroke={t.foreground} strokeWidth={1.25} />)}
    </>
  )
}

/** The TS twin of the Rust lowering's shape (tables as `name  type  key` rows, then relations), for the time-out. */
export function entitiesText(p: EntitiesData): string {
  const detail = p.detail ?? 'model'
  const name = (id: string) => p.entities.find((e) => e.id === id)?.label ?? id
  const out: string[] = []
  if (p.title) out.push(p.title, '')
  for (const e of p.entities) {
    out.push(e.label)
    for (const f of e.fields.slice(0, SHOWN_FIELDS)) out.push(`  ${fieldColumns(f, detail).filter(Boolean).join('  ')}`)
    if (e.fields.length > SHOWN_FIELDS) out.push(`  +${e.fields.length - SHOWN_FIELDS} more`)
    out.push('')
  }
  for (const r of p.relations) out.push(`${name(r.from)} → ${name(r.to)}  (${r.kind}${r.from_field && r.to_field ? `: ${r.from_field} → ${r.to_field}` : ''}${r.label ? `, ${r.label}` : ''})`)
  return out.join('\n').trimEnd()
}

export function entitiesTooltip(p: EntitiesData, hover: string | null): Tip | null {
  if (hover === null) return null
  if (hover.startsWith('edge:')) {
    const r = p.relations[Number(hover.slice(5))]
    if (!r) return null
    const name = (id: string) => p.entities.find((e) => e.id === id)?.label ?? id
    return { title: `${name(r.from)} → ${name(r.to)}`, rows: [{ label: 'kind', value: r.kind }, { label: 'label', value: r.label }, { label: 'fields', value: r.from_field && r.to_field ? `${r.from_field} → ${r.to_field}` : null }] }
  }
  const e = p.entities.find((x) => x.id === hover)
  return e ? { title: e.label, rows: [{ label: 'kind', value: e.kind }, { label: 'fields', value: e.fields.length }, { label: 'state', value: e.state }, { label: 'ref', value: e.ref }] } : null
}

/** The svg at `layout`. Draw order: relations → boxes → labels. */
export function EntitiesSvg({ props: p, layout: l, tokens: t, markerId, units, root }: { props: EntitiesData; layout: Layout; tokens: DiagramTokens; markerId: string; units?: Units; root?: { onKeyDown: (e: React.KeyboardEvent) => void } }) {
  const detail = p.detail ?? 'model'
  const top = MARGIN + (p.title ? TITLE_H : 0)
  const W = Math.ceil(l.width + 2 * MARGIN)
  const H = Math.ceil(top + l.height + MARGIN)
  const a = svgA11y(markerId, !!units)
  const entities: Record<string, Entity> = {}
  for (const e of p.entities) entities[e.id] = e
  const name = (id: string) => entities[id]?.label ?? id
  return (
    <svg xmlns="http://www.w3.org/2000/svg" viewBox={`0 0 ${W} ${H}`} width={W} height={H} style={{ maxWidth: '100%', height: 'auto' }} {...a.svg} {...root}>
      <title id={a.titleId}>{p.title ?? 'Entities'}</title>
      <desc id={a.descId}>{describe('Entities', p.title, p.entities.length, 'entities')}</desc>
      <defs>
        <RelationMarkers id={markerId} tokens={t} />
      </defs>
      {p.title && (
        <text x={MARGIN} y={MARGIN + 13} fontFamily={t.font} fontSize={FONT_PX} fontWeight={600} fill={t.foreground}>
          {p.title}
        </text>
      )}
      <g transform={`translate(${MARGIN} ${top})`}>
        {l.edges.map((e) => {
          const r = p.relations[e.index]
          if (!r) return null
          const st: UnitState = units ? units.paint(edgeUnit(e.index), null) : 'default'
          const pt = paint(st, t)
          const marks = relationMarks(r.kind)
          const d = sectionsPath(e.sections)
          const at = r.label ? edgeLabelAt(e) : null
          return (
            <g key={e.index} opacity={pt.opacity} data-unit={edgeUnit(e.index)} data-relation={r.kind} {...(units ? units.unit(edgeUnit(e.index), `${name(r.from)} → ${name(r.to)}: ${r.kind}${r.label ? `, ${r.label}` : ''}`) : {})}>
              <path d={d} fill="none" stroke="transparent" strokeWidth={18} />
              <path d={d} fill="none" stroke={pt.stroke} strokeWidth={st === 'active' ? 2 : 1.25} strokeDasharray={marks.dashed ? '6 4' : undefined} markerStart={marks.start ? `url(#${markerId}-${marks.start})` : undefined} markerEnd={marks.end ? `url(#${markerId}-${marks.end})` : undefined} />
              {at && r.label && <MaskedLabel text={r.label} at={at.at} along={at.along} side={at.side} tokens={t} />}
            </g>
          )
        })}
        {l.nodes.map((n) => {
          const e = entities[n.id]
          if (!e) return null
          const box = entityBox(e, detail)
          const st = units ? units.paint(n.id, e.state) : (e.state ?? 'default')
          const pt = paint(st, t)
          const fieldsTop = n.y + HEAD_H
          const methodsTop = fieldsTop + (box.shown + (box.more ? 1 : 0) ? (box.shown + (box.more ? 1 : 0)) * ROW_H + 8 : 0)
          const col1 = n.x + PAD_X
          const col2 = col1 + box.cols[0]
          const col3 = n.x + n.w - PAD_X
          return (
            <g key={n.id} opacity={pt.opacity} data-unit={n.id} data-kind={e.kind ?? 'entity'} aria-description={e.ref ?? undefined} {...(units ? units.unit(n.id, unitLabel(e.label, e.kind, st)) : {})}>
              <rect x={n.x} y={n.y} width={n.w} height={n.h} rx={4} fill={pt.fill} fillOpacity={pt.fillOpacity} stroke={pt.stroke} strokeWidth={pt.strokeWidth} />
              <text x={col1} y={n.y + HEAD_H / 2} dominantBaseline="central" fontFamily={t.font} fontSize={FONT_PX} fontWeight={600} fill={t.foreground}>
                {e.label}
              </text>
              {e.kind && e.kind !== 'entity' && (
                <text x={col3} y={n.y + HEAD_H / 2} dominantBaseline="central" textAnchor="end" fontFamily={MONO} fontSize={MONO_PX} fill={t.mutedForeground}>
                  {e.kind}
                </text>
              )}
              {(box.shown > 0 || box.more > 0) && <line x1={n.x} y1={fieldsTop} x2={n.x + n.w} y2={fieldsTop} stroke={pt.stroke} strokeWidth={SEP} />}
              {e.fields.slice(0, box.shown).map((f, i) => {
                const [c1, c2, c3] = fieldColumns(f, detail)
                const y = fieldsTop + 4 + i * ROW_H + ROW_H / 2
                return (
                  <g key={f.name} data-field={f.name}>
                    <text x={col1} y={y} dominantBaseline="central" fontFamily={MONO} fontSize={MONO_PX} fill={t.foreground} fontWeight={f.key === 'pk' ? 600 : undefined}>
                      {c1}
                    </text>
                    {c2 && (
                      <text x={col2} y={y} dominantBaseline="central" fontFamily={MONO} fontSize={MONO_PX} fill={t.mutedForeground}>
                        {c2}
                      </text>
                    )}
                    {c3 && (
                      <text x={col3} y={y} dominantBaseline="central" textAnchor="end" fontFamily={MONO} fontSize={MONO_PX} fill={t.mutedForeground}>
                        {c3}
                      </text>
                    )}
                  </g>
                )
              })}
              {box.more > 0 && (
                <text x={col1} y={fieldsTop + 4 + box.shown * ROW_H + ROW_H / 2} dominantBaseline="central" fontFamily={MONO} fontSize={MONO_PX} fill={t.mutedForeground} data-more={box.more}>
                  +{box.more} more
                </text>
              )}
              {box.methods.length > 0 && (
                <g data-methods>
                  <line x1={n.x} y1={methodsTop} x2={n.x + n.w} y2={methodsTop} stroke={pt.stroke} strokeWidth={SEP} />
                  {box.methods.map((m, i) => (
                    <text key={m} x={col1} y={methodsTop + 4 + i * ROW_H + ROW_H / 2} dominantBaseline="central" fontFamily={MONO} fontSize={MONO_PX} fill={t.foreground}>
                      {m}
                    </text>
                  ))}
                </g>
              )}
            </g>
          )
        })}
      </g>
    </svg>
  )
}

export function EntitiesView({ props, loading, highlight, onSelect }: DiagramHostProps<EntitiesData>) {
  const p = useMemo(() => (loading ? dropDangling(props) : props), [props, loading])
  const hash = useMemo(() => structuralHash(p), [p])
  const tokens = useDiagramTokens()
  const shown = useElkLayout(hash, () => toElk(p), flattenElk)
  const drawnRef = useRef(p)
  const drawn = shown && shown.hash === hash ? p : drawnRef.current
  useEffect(() => {
    drawnRef.current = drawn
  }, [drawn])
  const ids = useMemo(() => drawn.entities.map((e) => e.id), [drawn])
  const units = useGraphUnits(ids, drawn.relations, highlight, onSelect)
  let content: ReactNode
  if (shown && 'layout' in shown) content = <EntitiesSvg props={drawn} layout={shown.layout} tokens={tokens} markerId={`en-${shown.hash}`} units={units} root={units.root} />
  else if (shown) content = <pre className="p-3 font-mono text-xs leading-5 whitespace-pre-wrap">{entitiesText(drawn)}</pre>
  return (
    <ElkFrame kind="entities" hash={hash} timeout={!!shown && 'timeout' in shown} hover={units.state.hover} tip={entitiesTooltip(drawn, units.state.hover)}>
      {content}
    </ElkFrame>
  )
}
