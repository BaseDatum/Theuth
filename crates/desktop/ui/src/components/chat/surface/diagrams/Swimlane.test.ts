import { readFileSync } from 'node:fs'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { fixedLayout } from '@/lib/graph-layout'
import { bands, dropDangling, type SwimlaneData, SwimlaneSvg, swimlaneText, toElk } from './Swimlane'

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
const fixture = (name: string): SwimlaneData => {
  const j = JSON.parse(readFileSync(new URL(`fixtures/json-render/${name}.json`, RUST), 'utf8'))
  return Object.values(j.elements as Record<string, { type: string; props: SwimlaneData }>).find((e) => e.type === 'Swimlane')!.props
}
const render = (p: SwimlaneData) => renderToStaticMarkup(createElement(SwimlaneSvg, { props: p, layout: fixedLayout(toElk(p)), tokens: TOKENS, markerId: 'sw' }))

test('valid-swimlane at a fixed layout matches its snapshot', () => {
  expect(render(fixture('valid-swimlane'))).toMatchSnapshot()
})

test('partitioning options name the lane index; bands tile the layout axis and cover every step of their lane', () => {
  const p = fixture('valid-swimlane')
  const elk = toElk(p)
  expect(elk.layoutOptions?.['org.eclipse.elk.partitioning.activate']).toBe('true')
  expect(elk.children?.find((c) => c.id === 'n:review')?.layoutOptions?.['org.eclipse.elk.partitioning.partition']).toBe('2')
  const l = fixedLayout(elk)
  const b = bands(p, l)
  expect(b.map((x) => x.id)).toEqual(['dev', 'ci', 'rev'])
  for (let i = 1; i < b.length; i++) expect(b[i]!.from).toBe(b[i - 1]!.to)
  for (const s of p.steps) {
    const n = l.nodes.find((x) => x.id === s.id)!
    const band = b.find((x) => x.id === s.lane)!
    expect(n.x, s.id).toBeGreaterThanOrEqual(band.from)
    expect(n.x + n.w, s.id).toBeLessThanOrEqual(band.to)
  }
})

test('step kinds draw as pill / diamond / parallelogram; lanes carry their label at the head; units everywhere', () => {
  const p = fixture('valid-swimlane')
  const html = render(p)
  expect(html.match(/data-lane="/g)?.length).toBe(3)
  expect(html).toContain('>Developer</text>')
  expect(html.match(/data-unit="/g)?.length).toBe(p.steps.length + p.edges.length)
  expect(html).toMatch(/data-kind="start"[^>]*>.*?<rect x="40" y="40" width="110" height="33" rx="16.5"/)
  expect(html).toMatch(/data-kind="decision"[^>]*>.*?<polygon /)
  const tb = render(fixture('valid-swimlane-tb'))
  expect(tb).toMatch(/data-kind="data"[^>]*>.*?<polygon points="[^"]+"/)
  expect(html).toMatch(/<desc id="sw-desc">Swimlane: Pull request flow — 6 steps<\/desc>/)
  expect(html).not.toContain('foreignObject')
})

test('streaming drops steps in unknown lanes and dangling edges; the text twin equals the Rust lowering', () => {
  const p = fixture('valid-swimlane')
  const open = { ...p, steps: [...p.steps, { id: 'x', lane: 'nope', label: 'X' }], edges: [...p.edges, { from: 'merge', to: 'x' }] }
  const d = dropDangling(open)
  expect(d.steps).toHaveLength(p.steps.length)
  expect(d.edges).toHaveLength(p.edges.length)
  const golden = readFileSync(new URL('tests/golden/valid-swimlane-w100.txt', RUST), 'utf8')
  expect(`${swimlaneText(p)}\n`).toBe(golden)
})
