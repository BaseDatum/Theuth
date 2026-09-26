import { readdirSync, readFileSync } from 'node:fs'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import type { DiagramTokens } from '@/lib/diagram-theme'
import type { GraphData, Layout } from '@/lib/graph-layout'
import { adjacencyText, GraphSvg } from './Graph'
import { parseSpec } from './parse'

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
}

/** The Graph element of a fixture body. */
function graphOf(name: string): GraphData {
  const spec = parseSpec(readFileSync(new URL(`${name}.json`, HERE), 'utf8'))
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
