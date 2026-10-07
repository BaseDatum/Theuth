import { readdirSync, readFileSync } from 'node:fs'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import type { DiagramTokens } from '@/lib/diagram-theme'
import type { GraphData, Layout } from '@/lib/graph-layout'
import { adjacencyText, edgeLabelAt, GraphSvg } from './Graph'
import { parseSpec } from './parse'
import { fixedLayout as elkFixed, toElk } from '@/lib/graph-layout'
import { labelBox, LABEL_GAP, truncate } from '@/lib/diagram/label'

const HERE = new URL('./__fixtures__/', import.meta.url)
const RUST = new URL('../../../../../../surface/fixtures/json-render/', import.meta.url)
const VALID = ['graph-architecture', 'graph-dependency-lr', 'graph-pipeline-tb']

const TOKENS: DiagramTokens = {
  background: '#e8dcc8',
  foreground: '#1a1612',
  primary: '#1a1612',
  muted: '#d4c4a8',
  mutedForeground: '#6b5e4f',
  border: '#d4c4a8',
  card: '#f3eadf',
  accent: '#b4531e',
  success: '#2f5d4a',
  danger: '#9b2d1f',
  font: 'sans-serif',
  series: ['#b4531e', '#2f5d4a', '#3b5b8c', '#8a6d1f', '#7a3e7a', '#6b5e4f'],
}

/** The Graph element of a fixture body (`__fixtures__`, or the surface crate's when `rust`). */
function graphOf(name: string, rust = false): GraphData {
  const spec = parseSpec(readFileSync(new URL(`${name}.json`, rust ? RUST : HERE), 'utf8'))
  if ('error' in spec) throw new Error(`${name}: ${spec.error}`)
  const el = Object.values(spec.spec.elements).find((e) => e.type === 'Graph')!
  return el.props as unknown as GraphData
}

/** What the worker would return: nodes on a diagonal, one straight section per edge, groups around their members. */
function fixedLayout(g: GraphData): Layout {
  const nodes = g.nodes.map((n, i) => ({ id: n.id, x: i * 120, y: i * 60, w: 100, h: 40 }))
  const at: Record<string, { x: number; y: number }> = {}
  for (const n of nodes) at[n.id] = { x: n.x + 50, y: n.y + 20 }
  return {
    nodes,
    edges: g.edges.map((e, index) => ({ index, sections: [{ start: at[e.from]!, bends: [], end: at[e.to]! }], ...(e.label ? { label: { x: 0, y: 0, w: 40, h: 18 } } : {}) })),
    groups: (g.groups ?? []).map((grp) => ({ id: grp.id, x: 0, y: 0, w: 400, h: 200 })),
    width: 120 * g.nodes.length,
    height: 60 * g.nodes.length,
  }
}

test('the fixture copies are byte-identical to the surface crate fixtures', () => {
  const names = readdirSync(HERE).filter((f) => f.startsWith('graph-'))
  expect(names.length).toBeGreaterThanOrEqual(5)
  for (const f of names) expect(readFileSync(new URL(f, HERE), 'utf8'), f).toBe(readFileSync(new URL(f, RUST), 'utf8'))
})

test.each(VALID)('%s renders every node, edge and group as svg', (name) => {
  const g = graphOf(name)
  const html = renderToStaticMarkup(createElement(GraphSvg, { graph: g, layout: fixedLayout(g), tokens: TOKENS, markerId: 'm' }))
  expect(html.startsWith('<svg')).toBe(true)
  expect(html.match(/data-node="/g)?.length).toBe(g.nodes.length)
  expect(html.match(/marker-end="url\(#m\)"/g)?.length).toBe(g.edges.length)
  expect(html.match(/rx="8" fill="#d4c4a8"/g)?.length ?? 0).toBe(g.groups?.length ?? 0)
  for (const n of g.nodes) expect(html).toContain(`>${n.label}</tspan>`)
  expect(html).not.toContain('foreignObject')
  expect(html.match(/stroke-dasharray="6 4"/g)?.length ?? 0).toBe(g.edges.filter((e) => e.style === 'dashed').length)
})

test('highlight paints active unless the node is done or danger; onSelect makes nodes buttons', () => {
  const g = graphOf('graph-architecture')
  const html = renderToStaticMarkup(
    createElement(GraphSvg, { graph: g, layout: fixedLayout(g), tokens: TOKENS, markerId: 'm', highlight: ['api', 'mailer'], onSelect: () => {} }),
  )
  const node = (id: string) => /<g[^>]*data-node="ID"[^>]*>(.*?)<\/g>/.exec(html.replace('ID', id).replace(`data-node="${id}"`, 'data-node="ID"'))?.[1] ?? ''
  expect(node('api')).toContain(`stroke="${TOKENS.accent}" stroke-width="2"`)
  expect(node('mailer')).toContain(`stroke="${TOKENS.danger}"`)
  expect(html.match(/role="button"/g)?.length).toBe(g.nodes.length)
})

test('closed-fence parsing rejects the broken graph fixtures at the Rust paths', () => {
  const at = (name: string) => {
    const r = parseSpec(readFileSync(new URL(`${name}.json`, HERE), 'utf8'))
    return 'error' in r ? r.error : 'ok'
  }
  expect(at('graph-broken-unknown-endpoint')).toBe('/elements/g/props/edges/1/to: unknown node "db2"')
  expect(at('graph-broken-dup-id')).toBe('/elements/g/props/nodes/2/id: duplicate node id "api"')
  const nodes = Array.from({ length: 121 }, (_, i) => ({ id: `n${i}`, label: 'x' }))
  const over = parseSpec(JSON.stringify({ root: 'g', elements: { g: { type: 'Graph', props: { nodes, edges: [] } } } }))
  expect('error' in over && over.error).toBe('/elements/g/props/nodes: 121 nodes, limit 120')
})

test('adjacencyText matches the surface crate list at unbounded width', () => {
  expect(adjacencyText(graphOf('graph-architecture'))).toBe(
    'Edge\n  • Browser → API (HTTPS), CDN\n  ○ CDN → API (origin)\nCore\n  • API → Worker (enqueue), Postgres (reads)\n  ◉ Worker → Postgres (writes), Mailer (notify)\n  • Postgres\n✗ Mailer',
  )
})

// ---- 123 §4.1 extensions (W1) ----

test('valid-graph-extended draws nested groups, boundary dashes, sub/tag/icon, the new kinds, edge state/arrow and the legend', () => {
  const g = graphOf('valid-graph-extended', true)
  const html = renderToStaticMarkup(createElement(GraphSvg, { graph: g, layout: elkFixed(toElk(g)), tokens: TOKENS, markerId: 'm' }))
  // Groups: three compound boxes, the `boundary` one dashed; nested order parent → child (draw order §5.4 #6).
  expect(html.match(/data-group="/g)?.length).toBe(3)
  expect(html.indexOf('data-group="cloud"')).toBeLessThan(html.indexOf('data-group="vpc"'))
  expect(html.indexOf('data-group="vpc"')).toBeLessThan(html.indexOf('data-group="db-net"'))
  expect(/data-group="cloud"><rect[^>]*stroke-dasharray="6 4"/.test(html)).toBe(true)
  // Nodes: every unit carries `data-unit` beside `data-node`; `sub` mono, `tag` rx=2, icons inline (no foreignObject).
  expect(html.match(/data-unit="/g)?.length).toBe(g.nodes.length + g.edges.length)
  expect(html.match(/data-node="/g)?.length).toBe(g.nodes.length)
  expect(html).toContain('>:8080</text>')
  expect(/<g data-tag="SERVICE"><rect[^>]*rx="2"/.test(html)).toBe(true)
  expect(html.match(/data-icon="/g)?.length).toBe(4)
  expect(html).not.toContain('foreignObject')
  // Kinds: hexagon and note are polygons/paths, circle a circle.
  const node = (id: string) => html.slice(html.indexOf(`data-unit="${id}"`), html.indexOf(`data-unit="${id}"`) + 2000)
  expect(node('cdn')).toMatch(/<polygon points="[^"]+" fill=/)
  expect(node('q')).toMatch(/<circle cx=/)
  expect(node('note')).toMatch(/<path d="M[^"]+Z M/)
  // Edges: `state: danger` paints the danger colour and heads with its marker; `arrow: both` sets marker-start; `none` sets neither.
  expect(html).toContain(`stroke="${TOKENS.danger}"`)
  expect(html.match(/marker-start="url\(#m\)"/g)?.length).toBe(1)
  expect(html.match(/marker-end="url\(#m\)"/g)?.length).toBe(2)
  expect(html).not.toContain('marker-end="url(#m-danger)"')
  // Legend: a bottom strip with one entry per legend item, below the drawing.
  expect(html.match(/data-legend="/g)?.length).toBe(3)
  expect(html.indexOf('data-legend=')).toBeGreaterThan(html.lastIndexOf('data-unit='))
  // Draw order groups → edges → nodes.
  expect(html.lastIndexOf('data-group=')).toBeLessThan(html.indexOf('marker-end='))
  expect(html.lastIndexOf('marker-end=')).toBeLessThan(html.indexOf('data-node='))
  // a11y: role img, title first, desc second.
  expect(html).toMatch(/<svg[^>]*role="img"[^>]*aria-labelledby="m-title m-desc"/)
  expect(html).toMatch(/<title id="m-title">Deployment<\/title><desc id="m-desc">Graph: Deployment — 5 nodes<\/desc>/)
})

test('edge labels sit on a mask off the stroke (§5.4 #2): mask ∩ stroke = ∅ for every labelled edge', () => {
  const g = graphOf('valid-graph-extended', true)
  const l = elkFixed(toElk(g))
  for (const e of l.edges) {
    const edge = g.edges[e.index]!
    if (!edge.label) continue
    const { at, along } = edgeLabelAt(e)
    const box = labelBox(truncate(edge.label))
    const cx = along === 'horizontal' ? at.x : at.x - (LABEL_GAP + box.w / 2)
    const cy = along === 'horizontal' ? at.y - (LABEL_GAP + box.h / 2) : at.y
    const mask = { x0: cx - box.w / 2, x1: cx + box.w / 2, y0: cy - box.h / 2, y1: cy + box.h / 2 }
    for (const s of e.sections) {
      const pts = [s.start, ...s.bends, s.end]
      for (let i = 1; i < pts.length; i++) {
        const a = pts[i - 1]!
        const b = pts[i]!
        const hits = Math.max(a.x, b.x) >= mask.x0 && Math.min(a.x, b.x) <= mask.x1 && Math.max(a.y, b.y) >= mask.y0 && Math.min(a.y, b.y) <= mask.y1
        expect(hits, `edge ${e.index} segment ${i} crosses its label mask`).toBe(false)
      }
    }
  }
})

test('adjacencyText prints sub, tag, nested groups and ✗→ like crates/surface::graph (the w100 golden)', () => {
  const golden = readFileSync(new URL('../../../../../../surface/tests/golden/valid-graph-extended-w100.txt', import.meta.url), 'utf8')
  expect(`${adjacencyText(graphOf('valid-graph-extended', true))}\n`).toBe(golden)
})
