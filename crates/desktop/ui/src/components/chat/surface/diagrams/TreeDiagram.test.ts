import { readFileSync } from 'node:fs'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { fixedLayout } from '@/lib/graph-layout'
import { forest, nested, toElk, type TreeData, TreeNestedSvg, TreeNodesSvg, treeText } from './TreeDiagram'

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
const trees = (name: string): TreeData[] => {
  const j = JSON.parse(readFileSync(new URL(`fixtures/json-render/${name}.json`, RUST), 'utf8'))
  return Object.values(j.elements as Record<string, { type: string; props: TreeData }>)
    .filter((e) => e.type === 'Tree')
    .map((e) => e.props)
}

test('valid-tree-nodes at a fixed layout matches its snapshot', () => {
  const p = trees('valid-tree-nodes')[0]!
  expect(renderToStaticMarkup(createElement(TreeNodesSvg, { props: p, layout: fixedLayout(toElk(p)), tokens: TOKENS, idBase: 'tree' }))).toMatchSnapshot()
})

test('rows nest under the nearest earlier shallower row; mrtree is the engine; connectors are orthogonal parent → child', () => {
  const p = trees('valid-tree-nodes')[0]!
  const f = forest(p.items)
  expect(f.map((n) => [n.id, n.parent])).toEqual([
    ['ws', null],
    ['agent', 'ws'],
    ['loop', 'agent'],
    ['surface', 'ws'],
    ['render', 'surface'],
    ['graph', 'surface'],
    ['desktop', 'ws'],
  ])
  expect(forest([{ label: 'a' }, { label: 'b', depth: 3 }])[1]).toMatchObject({ id: 'i:1', parent: 'i:0', depth: 1 })
  const elk = toElk(p)
  expect(elk.layoutOptions?.['elk.algorithm']).toBe('mrtree')
  expect(elk.edges).toHaveLength(6)
  const html = renderToStaticMarkup(createElement(TreeNodesSvg, { props: p, layout: fixedLayout(elk), tokens: TOKENS, idBase: 'tree' }))
  expect(html.match(/data-connector="/g)?.length).toBe(6)
  expect(html.match(/data-unit="/g)?.length).toBe(7)
  expect(html).toMatch(/data-connector="agent" d="M[\d. ]+ L[\d. ]+ Q/)
  expect(html).toMatch(/<desc id="tree-desc">Tree: Crates — 7 items<\/desc>/)
})

test('org cards carry detail as the role line; nested boxes contain their children without an engine', () => {
  const [org, nest] = trees('valid-tree-layouts')
  const html = renderToStaticMarkup(createElement(TreeNodesSvg, { props: org!, layout: fixedLayout(toElk(org!)), tokens: TOKENS, idBase: 'org' }))
  expect(html).toContain('>CTO</text>')
  expect(html).toContain('aria-description="docs/org.md"')
  const n = nested(forest(nest!.items))
  const ws = n.boxes[0]!
  expect(ws.node.item.label).toBe('Workspace')
  for (const c of ws.children) {
    expect(c.x).toBeGreaterThan(ws.x)
    expect(c.y).toBeGreaterThan(ws.y)
    expect(c.x + c.w).toBeLessThan(ws.x + ws.w)
    expect(c.y + c.h).toBeLessThan(ws.y + ws.h)
  }
  const nh = renderToStaticMarkup(createElement(TreeNestedSvg, { props: nest!, tokens: TOKENS, idBase: 'nested' }))
  expect(nh.match(/data-depth="2"/g)?.length).toBeGreaterThan(0)
  expect(nh).toContain('>json-render</text>')
  expect(treeText(org!)).toBe('Org\n├ CEO\n│ ├ CTO\n│ │ ├ Engineering\n│ ├ CFO')
})
