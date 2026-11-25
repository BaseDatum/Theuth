import { readFileSync } from 'node:fs'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { fixedLayout } from '@/lib/graph-layout'
import { type EntitiesData, EntitiesSvg, entityBox, fieldY, portId, relationMarks, SHOWN_FIELDS, toElk } from './Entities'

const RUST = new URL('../../../../../../../surface/', import.meta.url)
const TOKENS: DiagramTokens = {
  background: '#fff',
  foreground: '#111',
  primary: '#111',
  muted: '#ddd',
  mutedForeground: '#666',
  border: '#ccc',
  card: '#fafafa',
  accent: '#1d5f82',
  success: '#265c47',
  danger: '#8f2f1f',
  font: 'sans-serif',
  series: ['#1d5f82', '#265c47', '#3b5b8c', '#8a6d1f', '#7a3e7a', '#6b5e4f'],
}
const fixture = (name: string): EntitiesData => {
  const j = JSON.parse(readFileSync(new URL(`fixtures/json-render/${name}.json`, RUST), 'utf8'))
  return Object.values(j.elements as Record<string, { type: string; props: EntitiesData }>).find((e) => e.type === 'Entities')!.props
}
const render = (p: EntitiesData) => renderToStaticMarkup(createElement(EntitiesSvg, { props: p, layout: fixedLayout(toElk(p)), tokens: TOKENS, markerId: 'en' }))

test('valid-entities at a fixed layout matches its snapshot', () => {
  expect(render(fixture('valid-entities'))).toMatchSnapshot()
})

test('a relation that names fields attaches through FIXED_POS ports at those rows; one without uses the box side', () => {
  const p = fixture('valid-entities')
  const elk = toElk(p)
  const user = elk.children!.find((c) => c.id === 'n:user')!
  expect(user.layoutOptions?.['org.eclipse.elk.portConstraints']).toBe('FIXED_POS')
  const port = user.ports!.find((x) => x.id === portId('user', 'id', 'from'))!
  expect(port.x).toBe(user.width)
  expect(port.y).toBe(fieldY(p.entities[0]!, 'id'))
  const session = elk.children!.find((c) => c.id === 'n:session')!
  expect(session.ports!.find((x) => x.id === portId('session', 'user_id', 'to'))!.x).toBe(0)
  expect(elk.edges![0]!.sources).toEqual([portId('user', 'id', 'from')])
  expect(elk.edges![0]!.targets).toEqual([portId('session', 'user_id', 'to')])
  // `turn → runner (depends)` names no field: the box itself.
  expect(elk.edges![2]!.sources).toEqual(['n:turn'])
  expect(elk.children!.find((c) => c.id === 'n:runner')!.layoutOptions?.['org.eclipse.elk.portConstraints']).toBe('FIXED_SIDE')
  // Two field ports on one side are rows apart (≥ 12 px, §5.4 #4).
  const ys = (elk.children!.find((c) => c.id === 'n:session')!.ports ?? []).map((x) => x.y!).sort((a, b) => a - b)
  for (let i = 1; i < ys.length; i++) expect(ys[i]! - ys[i - 1]!).toBeGreaterThanOrEqual(12)
})

test('schema detail shows type and key/nullable columns in mono; crow\'s-foot and UML markers by kind', () => {
  const html = render(fixture('valid-entities'))
  expect(html).toMatch(/data-field="user_id">.*?>user_id<\/text><text [^>]*>uuid<\/text><text [^>]*text-anchor="end"[^>]*>fk<\/text>/)
  expect(html).toMatch(/data-field="title">.*?>null<\/text>/)
  expect(html).toContain('data-relation="one-many"')
  expect(html).toContain('marker-start="url(#en-bar)"')
  expect(html).toContain('marker-end="url(#en-crow)"')
  expect(html).toMatch(/data-relation="depends"[^>]*>.*?stroke-dasharray="6 4"[^>]*marker-end="url\(#en-open\)"/)
  expect(html).toContain('<title>has</title>')
  expect(html).toMatch(/<desc id="en-desc">Entities: Sessions schema — 4 entities<\/desc>/)
  expect(relationMarks('inherits')).toEqual({ start: null, end: 'tri', dashed: false })
  expect(relationMarks('composes')).toEqual({ start: 'diaf', end: null, dashed: false })
  expect(html).not.toContain('foreignObject')
})

test('class detail adds the methods compartment and caps fields at 12 with "+n more"', () => {
  const p = fixture('valid-entities-class')
  const worker = p.entities.find((e) => e.id === 'worker')!
  const box = entityBox(worker, 'class')
  expect(box.shown).toBe(SHOWN_FIELDS)
  expect(box.more).toBe(2)
  expect(box.methods).toEqual(['run(turn) -> Result', 'stop()'])
  expect(fieldY(worker, 'f14')).toBeNull()
  const html = render(p)
  expect(html).toContain('data-more="2">+2 more</text>')
  expect(html).toMatch(/<g data-methods="true">.*?>run\(turn\) -&gt; Result<\/text>/)
  expect(html).toContain('marker-end="url(#en-tri)"')
  expect(html).toContain('marker-start="url(#en-diaf)"')
  expect(html).toContain('marker-start="url(#en-dia)"')
})
