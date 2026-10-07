import { readFileSync } from 'node:fs'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { fixedLayout, toElk } from '@/lib/graph-layout'
import { type GraphDeltaData, GraphDeltaSvg, graphDeltaText, ledger, sideLayout, union } from './GraphDelta'

const RUST = new URL('../../../../../../../surface/', import.meta.url)
const TOKENS: DiagramTokens = {
  background: '#fff',
  foreground: '#111',
  primary: '#111',
  muted: '#ddd',
  mutedForeground: '#666',
  border: '#ccc',
  card: '#fafafa',
  accent: '#b4531e',
  success: '#2f5d4a',
  danger: '#9b2d1f',
  font: 'sans-serif',
  series: ['#b4531e', '#2f5d4a', '#3b5b8c', '#8a6d1f', '#7a3e7a', '#6b5e4f'],
}
const fixture = (name: string): GraphDeltaData => {
  const j = JSON.parse(readFileSync(new URL(`fixtures/json-render/${name}.json`, RUST), 'utf8'))
  return Object.values(j.elements as Record<string, { type: string; props: GraphDeltaData }>).find((e) => e.type === 'GraphDelta')!.props
}
const render = (p: GraphDeltaData) => renderToStaticMarkup(createElement(GraphDeltaSvg, { props: p, layout: fixedLayout(toElk(union(p))), tokens: TOKENS, markerId: 'gd' }))

test('valid-graph-delta at a fixed layout matches its snapshot', () => {
  expect(render(fixture('valid-graph-delta'))).toMatchSnapshot()
})

test('an unchanged node has identical coordinates on both panels (one union layout, ADR 0012)', () => {
  const p = fixture('valid-graph-delta')
  const u = union(p)
  const l = fixedLayout(toElk(u))
  const before = sideLayout(p.before, u, l).nodes.find((n) => n.id === 'web')!
  const after = sideLayout(p.after, u, l).nodes.find((n) => n.id === 'web')!
  expect(before).toEqual(after)
  const html = render(p)
  const panel = (name: string) => html.slice(html.indexOf(`data-panel="${name}"`), name === 'before' ? html.indexOf('data-panel="after"') : html.indexOf('data-ledger'))
  const rect = (s: string, id: string) => /<rect x="([\d.]+)" y="([\d.]+)"/.exec(s.slice(s.indexOf(`data-unit="${id}"`)))!.slice(1, 3)
  expect(rect(panel('before'), 'web')).toEqual(rect(panel('after'), 'web'))
  // Each panel draws only its side: `cache` is after-only, the group too.
  expect(panel('before')).not.toContain('data-unit="cache"')
  expect(panel('after')).toContain('data-unit="cache"')
  expect(panel('before')).not.toContain('data-group="data"')
  expect(panel('after')).toContain('data-group="data"')
})

test('the ledger is derived by id (label, kind, group) and drawn under the panels', () => {
  expect(ledger(fixture('valid-graph-delta'))).toEqual({ added: ['cache'], removed: [], changed: [] })
  const g = fixture('valid-graph-delta-groups')
  expect(ledger(g)).toEqual({ added: [], removed: ['legacy'], changed: [{ id: 'db', what: 'kind, group' }, { id: 'api', what: 'label' }] })
  const html = render(g)
  const strip = html.slice(html.indexOf('data-ledger'))
  expect(strip).toContain('− Legacy worker')
  expect(strip).toContain('~ Postgres (kind, group)')
  expect(strip).toContain('~ API v2 (label)')
  expect(html).toMatch(/<desc id="gd-desc">GraphDelta: 2 nodes<\/desc>/)
})

test('the text twin equals the Rust lowering (w100 golden)', () => {
  for (const name of ['valid-graph-delta', 'valid-graph-delta-groups']) {
    const golden = readFileSync(new URL(`tests/golden/${name}-w100.txt`, RUST), 'utf8')
    expect(`${graphDeltaText(fixture(name))}\n`, name).toBe(golden)
  }
})
