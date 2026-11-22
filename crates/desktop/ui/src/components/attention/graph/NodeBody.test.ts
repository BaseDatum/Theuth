// 208 §10.2: a node's content per level of detail, from the fixture.
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import type { GraphNode } from '@/rpc/bindings/method.agent.output'
import { GRAPH_FIXTURE, GRAPH_FIXTURE_ATTENTION as ATT, GRAPH_FIXTURE_NOW as NOW } from './fixture'
import { type Lod, NodeBody } from './NodeBody'

const node = (id: string): GraphNode => GRAPH_FIXTURE.nodes.find((n) => n.id === id) as GraphNode
const render = (id: string, lod: Lod = 'full', extra: { selected?: boolean; highlighted?: boolean } = {}) =>
  renderToStaticMarkup(createElement(NodeBody, { node: node(id), lod, attention: ATT.get(id) ?? 0, highlighted: false, selected: false, opacity: 1, now: NOW, ...extra }))
const text = (html: string) => html.replace(/<[^>]+>/g, '')

test('full: r1.builder — approval text, badge 1, the danger ring, data attributes and aria-label', () => {
  const html = render('r1.builder')
  expect(text(html)).toContain('approval: bash')
  expect(html).toMatch(/ring-danger/)
  expect(html).toMatch(/bg-danger[^"]*">1</)
  expect(html).toContain('data-node-id="r1.builder"')
  expect(html).toContain('data-lod="full"')
  expect(html).toContain('aria-label="builder · approval: bash"')
  expect(text(html)).toContain('opus-5.5')
  expect(text(html)).toContain('1m')
})

test('full: harness glyph for the claude child, eye mark for the observed one, remote chip, job counts', () => {
  const claude = render('r1.claude')
  const theuth = render('r1.docs')
  // The harness glyph is an extra svg beside the state glyph: present for claude, absent for a theuth child.
  expect((claude.match(/<svg/g) ?? []).length).toBeGreaterThan((theuth.match(/<svg/g) ?? []).length)
  expect(render('r1.claude.explore')).toContain('aria-label="watch-only"')
  expect(text(render('r6'))).toContain('remote · mini')
  expect(text(render('r1.builder.tests'))).toContain('waiting on a job · opus-5.5 · 2 jobs · 3m')
  expect(render('r1.docs')).not.toMatch(/ring-danger/)
})

test('compact: glyph, name and badge only; dot: no text at all', () => {
  const compact = render('r7', 'compact')
  expect(text(compact)).toContain('concierge desk')
  expect(text(compact)).toContain('2')
  expect(text(compact)).not.toContain('finished')
  expect(compact).toContain('data-lod="compact"')
  const dot = render('r1.builder', 'dot')
  expect(text(dot).trim()).toBe('')
  expect(dot).toContain('data-lod="dot"')
  expect(dot).toMatch(/ring-danger/)
})

test('selection and highlight use the accent border', () => {
  expect(render('r1.docs', 'full', { selected: true })).toContain('border-fg-accent')
  expect(render('r1.docs', 'full', { highlighted: true })).toContain('border-fg-accent')
  expect(render('r1.docs')).not.toContain('border-fg-accent')
})

test('no emoji in any node at any level of detail', () => {
  const emoji = /\p{Extended_Pictographic}/u
  for (const n of GRAPH_FIXTURE.nodes)
    for (const lod of ['full', 'compact', 'dot'] as const) {
      const t = text(render(n.id, lod)).replace(/[\u2190-\u21FF\u2713\u2717\u25A0\u25CB\u25CF]/g, '')
      expect([n.id, lod, emoji.test(t)]).toEqual([n.id, lod, false])
    }
})
