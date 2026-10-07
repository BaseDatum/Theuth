// Design 123 §4.1, §5.2 (W1): `Tree` with `layout: nodes | org | nested`. `nodes` and `org` are a top-down tidy tree
// through `org.eclipse.elk.mrtree` (shipped in elkjs 0.9.3 — verified in elk-worker.min.js) in the worker; the
// connectors are drawn here (parent bottom → child top, one r=8 elbow pair), not from ELK's sections, so they are
// orthogonal whatever mrtree's routing mode. `org` nodes are cards with `detail` as the role line. `nested` is
// recursive containment boxes measured synchronously — no engine. `list` stays the registry's HTML rows.
import type { ElkNode } from 'elkjs/lib/elk-api.js'
import { type ReactNode, useEffect, useMemo, useRef } from 'react'
import type { z } from 'zod'
import { describe, svgA11y, unitLabel } from '@/lib/diagram/a11y'
import { fnv, norm, PAD_X, textWidth } from '@/lib/diagram/elk'
import { orthogonalPath } from '@/lib/diagram/path'
import type { DiagramHostProps, UnitState } from '@/lib/diagram/types'
import { type Units, useUnits } from '@/lib/diagram/units'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { flattenElk, FONT_PX, type Layout, LINE_PX, nodeSize, useDiagramTokens, useElkLayout } from '@/lib/graph-layout'
import { ElkFrame, Label, MARGIN, MONO, paint, Shape, type Tip, TITLE_H } from '../Graph'
import type { catalog } from '../catalog'

export type TreeData = z.infer<(typeof catalog)['data']['components']['Tree']['props']>
type Item = TreeData['items'][number]

/** One row with its unit id (`id` or `i:<index>`) and its parent (the nearest earlier row of smaller depth). */
export interface TreeNode {
  id: string
  index: number
  item: Item
  parent: string | null
  depth: number
}

/** The rows as a forest: depth clamps to parent + 1 so a skipped level still nests (as the TUI's indent does). */
export function forest(items: readonly Item[]): TreeNode[] {
  const out: TreeNode[] = []
  const stack: TreeNode[] = []
  items.forEach((item, index) => {
    const want = item.depth ?? 0
    while (stack.length && stack[stack.length - 1]!.depth >= want) stack.pop()
    const parent = stack[stack.length - 1] ?? null
    const node: TreeNode = { id: item.id ?? `i:${index}`, index, item, parent: parent?.id ?? null, depth: parent ? parent.depth + 1 : 0 }
    out.push(node)
    stack.push(node)
  })
  return out
}

export function structuralHash(p: TreeData): string {
  return fnv([`tree:${p.layout ?? 'list'}:${norm(p.title)}`, ...p.items.map((i, k) => `i\u0001${i.id ?? k}\u0001${i.depth ?? 0}\u0001${norm(i.label)}\u0001${norm(i.detail)}\u0001${i.state ?? ''}`)].join('\u0002'))
}

const ORG_W = 144

function itemSize(n: TreeNode, org: boolean): { w: number; h: number } {
  if (org) return { w: ORG_W, h: n.item.detail ? 48 : 36 }
  return nodeSize(n.item.label, 0)
}

/** The forest as an mrtree graph: every row a node, every parent → child link an edge. */
export function toElk(p: TreeData): ElkNode {
  const org = p.layout === 'org'
  const nodes = forest(p.items)
  return {
    id: 'root',
    layoutOptions: { 'elk.algorithm': 'mrtree', 'elk.direction': 'DOWN', 'elk.spacing.nodeNode': org ? '24' : '20', 'org.eclipse.elk.mrtree.edgeRoutingMode': 'AVOID_OVERLAP' },
    children: nodes.map((n) => {
      const s = itemSize(n, org)
      return { id: `n:${n.id}`, width: s.w, height: s.h }
    }),
    edges: nodes.filter((n) => n.parent !== null).map((n) => ({ id: `e:${n.index}`, sources: [`n:${n.parent}`], targets: [`n:${n.id}`] })),
  }
}

/** Nested boxes: a leaf is one label line, a parent is a header over its children stacked vertically. */
export interface NestedBox {
  node: TreeNode
  x: number
  y: number
  w: number
  h: number
  children: NestedBox[]
}
const N_PAD = 8
const N_HEAD = 22
const N_GAP = 6

/** Measure and place the nested boxes of `nodes` (siblings share their parent's inner width). */
export function nested(nodes: readonly TreeNode[]): { boxes: NestedBox[]; width: number; height: number } {
  const kids = (id: string | null) => nodes.filter((n) => n.parent === id)
  const measure = (n: TreeNode): NestedBox => {
    const children = kids(n.id).map(measure)
    const textW = textWidth(n.item.label, FONT_PX) + (n.item.detail ? textWidth(n.item.detail, 11) + 12 : 0) + 2 * N_PAD
    if (children.length === 0) return { node: n, x: 0, y: 0, w: Math.max(80, Math.ceil(textW / 4) * 4), h: N_HEAD + 2, children }
    const inner = Math.max(textW, ...children.map((c) => c.w))
    let y = N_HEAD + N_GAP
    for (const c of children) {
      c.x = N_PAD
      c.y = y
      c.w = inner
      y += c.h + N_GAP
    }
    return { node: n, x: 0, y: 0, w: inner + 2 * N_PAD, h: y + N_PAD - N_GAP, children }
  }
  const place = (b: NestedBox, ox: number, oy: number) => {
    b.x += ox
    b.y += oy
    for (const c of b.children) place(c, b.x, b.y)
  }
  // Widening a parent to its siblings' width must reach its children too.
  const widen = (b: NestedBox) => {
    for (const c of b.children) {
      c.w = b.w - 2 * N_PAD
      widen(c)
    }
  }
  const roots = kids(null).map(measure)
  const width = Math.max(0, ...roots.map((r) => r.w))
  let y = 0
  for (const r of roots) {
    r.w = width
    widen(r)
    place(r, 0, y)
    y += r.h + N_GAP
  }
  return { boxes: roots, width, height: Math.max(0, y - N_GAP) }
}

/** The indented list the TUI prints — the time-out text. */
export function treeText(p: TreeData): string {
  return [p.title, ...p.items.map((i) => `${'│ '.repeat(i.depth ?? 0)}├ ${i.label}${i.detail ? `  ${i.detail}` : ''}`)].filter(Boolean).join('\n')
}

export function treeTooltip(nodes: readonly TreeNode[], hover: string | null): Tip | null {
  const n = hover === null ? undefined : nodes.find((x) => x.id === hover)
  return n ? { title: n.item.label, rows: [{ label: 'detail', value: n.item.detail }, { label: 'state', value: n.item.state }, { label: 'ref', value: n.item.ref }] } : null
}

function unitAttrs(units: Units | undefined, n: TreeNode, st: UnitState) {
  return units ? units.unit(n.id, unitLabel(n.item.label, n.item.detail, st)) : {}
}

/** The `nodes` / `org` svg at `layout`. Draw order: connectors → nodes → labels. */
export function TreeNodesSvg({ props: p, layout: l, tokens: t, units, root, idBase }: { props: TreeData; layout: Layout; tokens: DiagramTokens; units?: Units; root?: { onKeyDown: (e: React.KeyboardEvent) => void }; idBase: string }) {
  const org = p.layout === 'org'
  const nodes = forest(p.items)
  const by: Record<string, TreeNode> = {}
  for (const n of nodes) by[n.id] = n
  const box: Record<string, Layout['nodes'][number]> = {}
  for (const n of l.nodes) box[n.id] = n
  const top = MARGIN + (p.title ? TITLE_H : 0)
  const W = Math.ceil(l.width + 2 * MARGIN)
  const H = Math.ceil(top + l.height + MARGIN)
  const a = svgA11y(idBase, !!units)
  return (
    <svg xmlns="http://www.w3.org/2000/svg" viewBox={`0 0 ${W} ${H}`} width={W} height={H} style={{ maxWidth: '100%', height: 'auto' }} {...a.svg} {...root}>
      <title id={a.titleId}>{p.title ?? 'Tree'}</title>
      <desc id={a.descId}>{describe('Tree', p.title, p.items.length, 'items')}</desc>
      {p.title && (
        <text x={MARGIN} y={MARGIN + 13} fontFamily={t.font} fontSize={FONT_PX} fontWeight={600} fill={t.foreground}>
          {p.title}
        </text>
      )}
      <g transform={`translate(${MARGIN} ${top})`}>
        {nodes.map((n) => {
          const c = box[n.id]
          const pb = n.parent === null ? undefined : box[n.parent]
          if (!c || !pb) return null
          const from = { x: pb.x + pb.w / 2, y: pb.y + pb.h }
          const to = { x: c.x + c.w / 2, y: c.y }
          const mid = (from.y + to.y) / 2
          const pts = from.x === to.x ? [from, to] : [from, { x: from.x, y: mid }, { x: to.x, y: mid }, to]
          const st = units ? units.paint(n.id, n.item.state) : (n.item.state ?? 'default')
          return <path key={`c:${n.id}`} data-connector={n.id} d={orthogonalPath(pts)} fill="none" stroke={paint(st === 'active' ? 'active' : 'default', t).stroke} strokeWidth={1.25} opacity={st === 'muted' ? 0.5 : 1} />
        })}
        {nodes.map((n) => {
          const c = box[n.id]
          if (!c) return null
          const st = units ? units.paint(n.id, n.item.state) : (n.item.state ?? 'default')
          const pt = paint(st, t)
          return (
            <g key={n.id} opacity={pt.opacity} data-unit={n.id} aria-description={n.item.ref ?? undefined} {...unitAttrs(units, n, st)}>
              <rect x={c.x - 4} y={c.y - 4} width={c.w + 8} height={c.h + 8} fill="transparent" />
              <Shape kind="rounded" x={c.x} y={c.y} w={c.w} h={c.h} fill={pt.fill} fillOpacity={pt.fillOpacity} stroke={pt.stroke} strokeWidth={pt.strokeWidth} />
              {org ? (
                <>
                  <text x={c.x + PAD_X} y={c.y + (n.item.detail ? 17 : c.h / 2)} dominantBaseline="central" fontFamily={t.font} fontSize={FONT_PX} fontWeight={600} fill={t.foreground}>
                    {n.item.label}
                  </text>
                  {n.item.detail && (
                    <text x={c.x + PAD_X} y={c.y + 17 + LINE_PX} dominantBaseline="central" fontFamily={MONO} fontSize={11} fill={t.mutedForeground}>
                      {n.item.detail}
                    </text>
                  )}
                </>
              ) : (
                <Label text={n.item.label} cx={c.x + c.w / 2} cy={c.y + c.h / 2} fill={t.foreground} t={t} weight={600} />
              )}
            </g>
          )
        })}
      </g>
    </svg>
  )
}

/** The `nested` svg: containment boxes, synchronous. */
export function TreeNestedSvg({ props: p, tokens: t, units, root, idBase }: { props: TreeData; tokens: DiagramTokens; units?: Units; root?: { onKeyDown: (e: React.KeyboardEvent) => void }; idBase: string }) {
  const nodes = forest(p.items)
  const n = nested(nodes)
  const top = MARGIN + (p.title ? TITLE_H : 0)
  const W = Math.ceil(n.width + 2 * MARGIN)
  const H = Math.ceil(top + n.height + MARGIN)
  const a = svgA11y(idBase, !!units)
  const draw = (b: NestedBox, depth: number): ReactNode => {
    const st = units ? units.paint(b.node.id, b.node.item.state) : (b.node.item.state ?? 'default')
    const pt = paint(st, t)
    return (
      <g key={b.node.id}>
        <g opacity={pt.opacity} data-unit={b.node.id} data-depth={depth} aria-description={b.node.item.ref ?? undefined} {...unitAttrs(units, b.node, st)}>
          <rect x={b.x} y={b.y} width={b.w} height={b.h} rx={6} fill={st === 'default' ? (depth % 2 ? t.card : t.muted) : pt.fill} fillOpacity={st === 'default' ? (depth % 2 ? 1 : 0.35) : pt.fillOpacity} stroke={pt.stroke} strokeWidth={pt.strokeWidth} />
          <text x={b.x + N_PAD} y={b.y + N_HEAD / 2 + 1} dominantBaseline="central" fontFamily={t.font} fontSize={FONT_PX} fontWeight={600} fill={t.foreground}>
            {b.node.item.label}
          </text>
          {b.node.item.detail && (
            <text x={b.x + b.w - N_PAD} y={b.y + N_HEAD / 2 + 1} dominantBaseline="central" textAnchor="end" fontFamily={MONO} fontSize={11} fill={t.mutedForeground}>
              {b.node.item.detail}
            </text>
          )}
        </g>
        {b.children.map((c) => draw(c, depth + 1))}
      </g>
    )
  }
  return (
    <svg xmlns="http://www.w3.org/2000/svg" viewBox={`0 0 ${W} ${H}`} width={W} height={H} style={{ maxWidth: '100%', height: 'auto' }} {...a.svg} {...root}>
      <title id={a.titleId}>{p.title ?? 'Tree'}</title>
      <desc id={a.descId}>{describe('Tree', p.title, p.items.length, 'items')}</desc>
      {p.title && (
        <text x={MARGIN} y={MARGIN + 13} fontFamily={t.font} fontSize={FONT_PX} fontWeight={600} fill={t.foreground}>
          {p.title}
        </text>
      )}
      <g transform={`translate(${MARGIN} ${top})`}>{n.boxes.map((b) => draw(b, 0))}</g>
    </svg>
  )
}

/** Units of a tree: rows in authored order; a row lights its parent and children; arrows walk authored order. */
function useTreeUnits(nodes: readonly TreeNode[], highlight?: readonly string[], onSelect?: (id: string) => void): Units {
  const ids = useMemo(() => nodes.map((n) => n.id), [nodes])
  const incident = useMemo(() => (id: string) => nodes.filter((n) => n.parent === id || (n.id === id && n.parent !== null)).map((n) => (n.id === id ? n.parent! : n.id)), [nodes])
  return useUnits({ ids, incident, highlight, onSelect })
}

export function TreeDiagramView({ props, highlight, onSelect }: DiagramHostProps<TreeData>) {
  const hash = useMemo(() => structuralHash(props), [props])
  const tokens = useDiagramTokens()
  const isNested = props.layout === 'nested'
  const shown = useElkLayout(hash, () => toElk(props), flattenElk, !isNested)
  const drawnRef = useRef(props)
  const drawn = isNested || (shown && shown.hash === hash) ? props : drawnRef.current
  useEffect(() => {
    drawnRef.current = drawn
  }, [drawn])
  const nodes = useMemo(() => forest(drawn.items), [drawn])
  const units = useTreeUnits(nodes, highlight, onSelect)
  let content: ReactNode
  if (isNested) content = <TreeNestedSvg props={drawn} tokens={tokens} units={units} root={units.root} idBase={`tree-${hash}`} />
  else if (shown && 'layout' in shown) content = <TreeNodesSvg props={drawn} layout={shown.layout} tokens={tokens} units={units} root={units.root} idBase={`tree-${shown.hash}`} />
  else if (shown) content = <pre className="p-3 font-mono text-xs leading-5 whitespace-pre-wrap">{treeText(drawn)}</pre>
  return (
    <ElkFrame kind="tree" hash={hash} timeout={!isNested && !!shown && 'timeout' in shown} hover={units.state.hover} tip={treeTooltip(nodes, units.state.hover)}>
      {content}
    </ElkFrame>
  )
}
