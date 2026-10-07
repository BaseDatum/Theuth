// 123 §5.3/§5.4 #13, §9 (closer) — the render proof: every `valid-*` fixture of the surface crate mounts through
// the real path (parse → registry → markup) without a React error, every drawn diagram sits in a `DiagramFrame`,
// every svg carries the a11y contract (`role="img"`, `<title>` first, `<desc>`, every `data-unit` a focusable
// button) and has units. The ELK family lays out in a worker (no effect runs in static markup), so its pure
// `<NameSvg>` is checked at the deterministic `fixedLayout` with the kernel's `useUnits`; the fence still has to
// yield its frame.
import { readdirSync, readFileSync } from 'node:fs'
import { type ComponentType, createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { useUnits } from '@/lib/diagram/units'
import { fixedLayout, toElk as graphToElk } from '@/lib/graph-layout'
import { GraphSvg } from '../Graph'
import { JsonRenderFence } from '../JsonRenderFence'
import { parseSpec } from '../parse'
import { EntitiesSvg, toElk as entitiesToElk } from './Entities'
import { GraphDeltaSvg, union } from './GraphDelta'
import { StateMachineSvg, toElk as stateToElk } from './StateMachine'
import { SwimlaneSvg, toElk as swimlaneToElk } from './Swimlane'
import { toElk as treeToElk, TreeNodesSvg } from './TreeDiagram'

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { message: vi.fn(), error: vi.fn() }) }))
vi.mock('@/lib/rpc', () => ({ useApi: () => ({}) }))
vi.mock('@/components/ui/tooltip', () => ({ Tooltip: ({ children }: { children: unknown }) => children, TooltipTrigger: ({ children }: { children: unknown }) => children, TooltipContent: () => null }))
vi.mock('@/lib/theme', () => ({ subscribeDark: () => () => {} }))
const TOKENS: DiagramTokens = { background: '#fff', foreground: '#111', primary: '#111', muted: '#ddd', mutedForeground: '#666', border: '#ccc', card: '#fafafa', accent: '#b4531e', success: '#2f5d4a', danger: '#9b2d1f', font: 'sans-serif', series: ['#b4531e', '#2f5d4a', '#3b5b8c', '#8a6d1f', '#7a3e7a', '#6b5e4f'] }
// hoisted above `TOKENS`: the factory must not close over it
vi.mock('@/lib/diagram-theme', () => ({
  diagramTokens: () => ({ background: '#fff', foreground: '#111', primary: '#111', muted: '#ddd', mutedForeground: '#666', border: '#ccc', card: '#fafafa', accent: '#b4531e', success: '#2f5d4a', danger: '#9b2d1f', font: 'sans-serif', series: ['#b4531e', '#2f5d4a', '#3b5b8c', '#8a6d1f', '#7a3e7a', '#6b5e4f'] }),
}))
const RUST = new URL('../../../../../../../surface/fixtures/json-render/', import.meta.url)

/** `kind` of the `DiagramFrame` each svg component mounts in (`data-streamdown="<kind>-block"`); ELK members lay out in the worker. */
const FRAME: Record<string, { kind: string; elk?: true }> = {
  Graph: { kind: 'graph', elk: true },
  Sequence: { kind: 'sequence' },
  StateMachine: { kind: 'statemachine', elk: true },
  Entities: { kind: 'entities', elk: true },
  Swimlane: { kind: 'swimlane', elk: true },
  GraphDelta: { kind: 'graphdelta', elk: true },
  BarChart: { kind: 'barchart' },
  LineChart: { kind: 'linechart' },
  ScatterChart: { kind: 'scatterchart' },
  RadarChart: { kind: 'radarchart' },
  Waterfall: { kind: 'waterfall' },
  Funnel: { kind: 'funnel' },
  Gantt: { kind: 'gantt' },
  Quadrant: { kind: 'quadrant' },
  Treemap: { kind: 'treemap' },
  Sankey: { kind: 'sankey' },
  Venn: { kind: 'venn' },
  Loop: { kind: 'loop' },
  Layers: { kind: 'layers' },
  Journey: { kind: 'journey' },
  Fishbone: { kind: 'fishbone' },
  Timeline: { kind: 'timeline' },
  Tree: { kind: 'tree' },
}
/** HTML diagrams (§5.2): units on `<div>`/`<td>`s, no svg, no frame. */
const HTML: Record<string, true> = { Kanban: true, Heatmap: true }
/** Lays out in the ELK worker (nothing drawn in static markup): the ELK family, and `Tree` except `nested`. */
const inWorker = (e: Element) => (e.type === 'Tree' ? e.props.layout !== 'nested' : !!FRAME[e.type]?.elk)

type Element = { type: string; props: Record<string, unknown> }
/** The svg-family elements of `spec` (Tree only when drawn, Timeline only on the time axis). */
function drawn(elements: Element[]): Element[] {
  return elements.filter((e) => {
    if (!FRAME[e.type]) return false
    if (e.type === 'Tree') return typeof e.props.layout === 'string' && e.props.layout !== 'list'
    if (e.type === 'Timeline') return e.props.scale === 'time'
    return true
  })
}

const FIXTURES = readdirSync(RUST)
  .filter((n) => n.startsWith('valid-') && n.endsWith('.json'))
  .sort()
  .map((n) => n.slice(0, -5))
const elementsOf = (name: string): Element[] => {
  const spec = parseSpec(readFileSync(new URL(`${name}.json`, RUST), 'utf8'))
  if ('error' in spec) throw new Error(`${name}: ${spec.error}`)
  return Object.values(spec.spec.elements) as Element[]
}
const fence = (name: string) => renderToStaticMarkup(createElement(JsonRenderFence, { code: readFileSync(new URL(`${name}.json`, RUST), 'utf8'), isIncomplete: false, language: 'json-render' }))

/** Every top-level labelled `<svg>` of `html` (`role="img"`, or `group` when its units are live buttons — `svgA11y`); nested icon svgs stay inside their parent. */
function svgs(html: string): string[] {
  const out: string[] = []
  const re = /<\/?svg\b/g
  let depth = 0
  let start = -1
  for (let m = re.exec(html); m; m = re.exec(html)) {
    if (m[0] === '<svg') {
      if (depth === 0) start = m.index
      depth++
    } else if (--depth === 0) {
      const end = html.indexOf('>', m.index) + 1
      const s = html.slice(start, end)
      if (/^<svg[^>]*\brole="(img|group)"/.test(s)) out.push(s)
    }
  }
  return out
}

/** §5.4 #13 + §5.3 #1 on one svg's markup. */
function expectAccessible(svg: string, where: string) {
  const open = /^<svg[^>]*>/.exec(svg)![0]
  const ids = /aria-labelledby="([^" ]+) ([^" ]+)"/.exec(open)
  expect(ids, `${where}: aria-labelledby title desc`).not.toBeNull()
  const body = svg.slice(open.length)
  expect(body.startsWith(`<title id="${ids![1]}">`), `${where}: <title> first`).toBe(true)
  expect(body, `${where}: <desc>`).toContain(`<desc id="${ids![2]}">`)
  const units = body.match(/<(?:g|div)\b[^>]*\bdata-unit="[^"]*"[^>]*>/g) ?? []
  expect(units.length, `${where}: units`).toBeGreaterThan(0)
  for (const u of units) {
    expect(u, `${where}: ${u} is a button`).toMatch(/\brole="button"/)
    expect(u, `${where}: ${u} is focusable`).toMatch(/\btabindex="0"/)
    expect(u, `${where}: ${u} is labelled`).toMatch(/\baria-label="[^"]+"/)
  }
}

test('the fixtures cover every svg component and both HTML diagrams', () => {
  const types = new Set(FIXTURES.flatMap((f) => drawn(elementsOf(f)).map((e) => e.type)))
  expect([...types].sort()).toEqual(Object.keys(FRAME).sort())
  const html = new Set(FIXTURES.flatMap((f) => elementsOf(f).filter((e) => HTML[e.type]).map((e) => e.type)))
  expect([...html].sort()).toEqual(['Heatmap', 'Kanban'])
})

test.each(FIXTURES)('%s mounts through the fence: a DiagramFrame per drawn diagram, accessible svgs with units', (name) => {
  const els = elementsOf(name)
  const html = fence(name)
  expect(html, `${name}: a component threw (the boundary's fallback)`).not.toContain('diagram omitted')
  expect(html).not.toContain('data-stub')
  const frames = drawn(els)
  for (const e of frames) expect(html, `${name}: ${e.type} in its DiagramFrame`).toContain(`data-streamdown="${FRAME[e.type]!.kind}-block"`)
  expect(html.match(/data-streamdown="[a-z]+-block"/g)?.length ?? 0, `${name}: one frame per drawn diagram`).toBe(frames.length)
  const sync = frames.filter((e) => !inWorker(e))
  const found = svgs(html)
  expect(found.length, `${name}: one svg per synchronous diagram`).toBe(sync.length)
  for (const s of found) expectAccessible(s, name)
  for (const e of els.filter((x) => HTML[x.type])) {
    const units = html.match(/<(?:div|td)\b[^>]*\bdata-unit="[^"]*"[^>]*>/g) ?? []
    expect(units.length, `${name}: ${e.type} units`).toBeGreaterThan(0)
    for (const u of units) expect(u).toMatch(/\brole="button"[^>]*\btabindex="0"/)
  }
})

/** The pure svg of an ELK member at the fixed layout, with the kernel's unit contract (a component so hooks run). */
const ELK_SVG: Record<string, (p: never) => { Svg: ComponentType<never>; props: Record<string, unknown> }> = {
  Graph: (graph: Parameters<typeof graphToElk>[0]) => ({ Svg: GraphSvg as ComponentType<never>, props: { graph, layout: fixedLayout(graphToElk(graph)) } }),
  StateMachine: (props: Parameters<typeof stateToElk>[0]) => ({ Svg: StateMachineSvg as ComponentType<never>, props: { props, layout: fixedLayout(stateToElk(props)) } }),
  Entities: (props: Parameters<typeof entitiesToElk>[0]) => ({ Svg: EntitiesSvg as ComponentType<never>, props: { props, layout: fixedLayout(entitiesToElk(props)) } }),
  Swimlane: (props: Parameters<typeof swimlaneToElk>[0]) => ({ Svg: SwimlaneSvg as ComponentType<never>, props: { props, layout: fixedLayout(swimlaneToElk(props)) } }),
  GraphDelta: (props: Parameters<typeof union>[0]) => ({ Svg: GraphDeltaSvg as ComponentType<never>, props: { props, layout: fixedLayout(graphToElk(union(props))) } }),
  Tree: (props: Parameters<typeof treeToElk>[0]) => ({ Svg: TreeNodesSvg as ComponentType<never>, props: { props, layout: fixedLayout(treeToElk(props)), idBase: 'tree' } }),
}
function Fixed({ Svg, props }: { Svg: ComponentType<never>; props: Record<string, unknown> }) {
  const units = useUnits({ ids: [] })
  return createElement(Svg, { ...props, tokens: TOKENS, markerId: 'm', units, root: units.root } as never)
}
const ELK_CASES = FIXTURES.flatMap((f) => drawn(elementsOf(f)).filter(inWorker).map((e) => [f, e.type, e.props] as const))

test.each(ELK_CASES)('%s: the %s svg at a fixed layout is accessible and every unit is a focusable button', (name, type, props) => {
  // `parseSpec` validated `props`; the builder narrows to its component
  const html = renderToStaticMarkup(createElement(Fixed, ELK_SVG[type]!(props as never)))
  const found = svgs(html)
  expect(found).toHaveLength(1)
  expectAccessible(found[0]!, `${name} ${type}`)
  expect((found[0]!.match(/data-unit="/g) ?? []).length, `${name}: ${type} draws a unit per node`).toBeGreaterThanOrEqual((props.nodes as unknown[] | undefined)?.length ?? 1)
  expect(found[0]!).not.toContain('foreignObject')
})

test('Tree dispatches on layout: list stays the HTML rows, nodes/org/nested mount the TreeDiagram frame', () => {
  const html = fence('valid-tree-layouts')
  expect(html.match(/data-streamdown="tree-block"/g)).toHaveLength(3)
  const list = renderToStaticMarkup(
    createElement(JsonRenderFence, {
      code: JSON.stringify({ root: 't', elements: { t: { type: 'Tree', props: { items: [{ label: 'a' }, { label: 'b', depth: 1 }], layout: 'list' } } } }),
      isIncomplete: false,
      language: 'json-render',
    }),
  )
  expect(list).not.toMatch(/data-streamdown="[a-z]+-block"/)
  expect(list).toContain('>a<')
})

test('Kanban and Heatmap keep their HTML forms (§5.2): no svg, no frame, units on divs/cells', () => {
  const kanban = fence('valid-kanban-rows')
  const heatmap = fence('valid-heatmap-diverging')
  expect(kanban).not.toContain('<svg role')
  expect(heatmap).toContain('color-mix(in oklab, var(--color-fg-accent)')
  expect(`${kanban}${heatmap}`).not.toMatch(/data-streamdown="[a-z]+-block"/)
})

test('Layout is deterministic: the same fixture renders the same markup twice', () => {
  for (const f of ['valid-bar-chart', 'valid-sankey', 'valid-gantt']) expect(fence(f)).toBe(fence(f))
})
