import { readFileSync } from 'node:fs'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { JsonRenderFence } from './JsonRenderFence'
import { parseSpec } from './parse'
import { activations, type SequenceData, sequenceDesc, SequenceSvg } from './Sequence'

vi.mock('@/lib/notify', () => ({ notify: Object.assign(vi.fn(), { message: vi.fn(), error: vi.fn() }) }))
vi.mock('@/lib/rpc', () => ({ useApi: () => ({}) }))
vi.mock('@/components/ui/tooltip', () => ({ Tooltip: ({ children }: { children: unknown }) => children, TooltipTrigger: ({ children }: { children: unknown }) => children, TooltipContent: () => null }))
vi.mock('@/lib/theme', () => ({ subscribeDark: () => () => {} }))
vi.mock('@/lib/diagram-theme', () => ({ diagramTokens: () => TOKENS }))

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
const RUST = new URL('../../../../../../surface/fixtures/json-render/', import.meta.url)
const load = (file: string): SequenceData => {
  const s = parseSpec(readFileSync(new URL(file, RUST), 'utf8'))
  if ('error' in s) throw new Error(s.error)
  return Object.values(s.spec.elements).find((e) => e.type === 'Sequence')!.props as unknown as SequenceData
}
const data = load('valid-sequence-fragments.json')
const svg = (d: SequenceData, extra: Record<string, unknown> = {}) => renderToStaticMarkup(createElement(SequenceSvg, { data: d, tokens: TOKENS, markerId: 'm', ...extra }))
const step = (html: string, i: number) => /<g data-step="I"[^>]*>(.*?)<\/g>/.exec(html.replace(`data-step="${i}"`, 'data-step="I"'))?.[1] ?? ''

test('snapshot over the fragments fixture', () => {
  expect(svg(data)).toMatchSnapshot()
})

test('unit contract: data-unit beside data-step on every step, title then desc, img without onSelect and group with', () => {
  const html = svg(data)
  expect(html.match(/<g data-step="\d+" role="button" tabindex="0" data-unit="\d+"/g)).toHaveLength(6)
  expect(html).toMatch(/^<svg[^>]*role="img"[^>]*aria-labelledby="m-title m-desc"/)
  expect(html).toContain('<title id="m-title">Fence correction round</title><desc id="m-desc">Sequence: Fence correction round — 6 steps</desc>')
  expect(sequenceDesc(data)).toBe('Sequence: Fence correction round — 6 steps')
  expect(html).toContain('aria-label="2. Loop → surface: check(text)"')
  expect(html).toContain('aria-description="crates/surface/src/lib.rs#L154"')
  expect(svg(data, { onSelect: () => {} })).toMatch(/^<svg[^>]*role="group"/)
})

test('fragments: two frames, the ALT tab with its else divider, the LOOP tab; streaming clamps `to` and drops a bad `from`', () => {
  const html = svg(data)
  expect(html.match(/data-fragment="\d"/g)).toHaveLength(2)
  expect(html).toContain('>ALT</text>')
  expect(html).toContain('>LOOP</text>')
  expect(html.match(/stroke-dasharray="4 3"/g)).toHaveLength(1)
  expect(html).toContain('>[else]</text>')
  expect(html).toContain('>errors</text>')
  const streamed = svg({ ...data, fragments: [{ kind: 'opt', from: 4, to: 99 }, { kind: 'par', from: 99, to: 100 }, { kind: 'alt', from: 0, to: 1, else: 5 }] })
  expect(streamed.match(/data-fragment="\d"/g)).toHaveLength(2)
  expect(streamed).not.toContain('stroke-dasharray="4 3"')
  const empty = svg({ actors: [], steps: [], fragments: [{ kind: 'loop', from: 0, to: 0 }] })
  expect(empty).toContain('<desc id="m-desc">Sequence: 0 steps</desc>')
  expect(empty).not.toContain('data-fragment')
})

test('notes sit beside the arrow with the full text in a title; state colours the arrow, head and dot', () => {
  const html = svg(data)
  expect(html).toContain('<title>path + message</title>path + message</text>')
  expect(html).toContain('<title>final=true</title>final=true</text>')
  const danger = step(html, 2)
  expect(danger).toContain('stroke="#8f2f1f" stroke-width="1.25" stroke-dasharray="6 4" marker-end="url(#m-closed-danger)"')
  expect(danger).toContain('r="5.5" fill="#8f2f1f" stroke="#8f2f1f"')
  const done = step(html, 4)
  expect(done).toContain('stroke="#265c47" stroke-width="1.25" marker-end="url(#m-closed-success)"')
  expect(step(html, 0)).toContain('stroke="#111" stroke-width="1.25"')
  // a step's own danger wins over a highlight elsewhere; the rest go muted
  const lit = svg(data, { highlight: [0] })
  expect(step(lit, 0)).toContain('stroke="#1d5f82" stroke-width="2.4"')
  expect(step(lit, 2)).toContain('stroke="#8f2f1f"')
  expect(step(lit, 1)).toContain('stroke="#666" stroke-width="1.25"')
})

test('activations: a call opens on the callee, the matching return closes it, an unmatched call runs to the end', () => {
  expect(activations(data)).toEqual([
    { actor: 'loop', from: 0, to: null },
    { actor: 'surface', from: 1, to: 2 },
    { actor: 'model', from: 3, to: null },
    { actor: 'loop', from: 4, to: null },
  ])
  expect(svg(data).match(/data-activation="/g)).toHaveLength(4)
  const self = { actors: [{ id: 'a', label: 'A' }], steps: [{ from: 'a', to: 'a', label: 'x' }, { from: 'a', to: 'a', label: 'y', style: 'async' as const }] }
  expect(activations(self)).toEqual([])
  // a step whose actor has not streamed yet draws no activation and does not throw
  const dangling = { actors: [{ id: 'a', label: 'A' }], steps: [{ from: 'a', to: 'ghost', label: 'x' }] }
  expect(activations(dangling)).toEqual([{ actor: 'ghost', from: 0, to: null }])
  expect(svg(dangling)).not.toContain('data-activation')
})

test('compatibility: the registry pins over valid-sequence.json and the 2-step highlight input', () => {
  const html = renderToStaticMarkup(createElement(JsonRenderFence, { code: readFileSync(new URL('valid-sequence.json', RUST), 'utf8'), isIncomplete: false, language: 'json-render' }))
  expect(html.match(/<g data-step="\d+" role="button" tabindex="0"/g)).toHaveLength(6)
  expect(html.match(/stroke-width="18"/g)).toHaveLength(6)
  expect(html).toContain('data-streamdown="sequence-block"')
  expect(html).not.toContain('data-stub')
  expect(html).toContain('Sequence: One json-render fence, model to picture — 6 steps</desc>')
  const two = { actors: [{ id: 'a', label: 'A' }, { id: 'b', label: 'B' }], steps: [{ from: 'a', to: 'b', label: 'x' }, { from: 'b', to: 'a', label: 'y', style: 'async' as const }] }
  const lit = svg(two, { highlight: [1], onSelect: () => {} })
  expect(lit.match(/cursor:pointer/g)).toHaveLength(2)
  expect(lit).toContain('<circle cx="276" cy="188"')
  expect(step(lit, 1)).toContain('marker-end="url(#m-open-accent)"')
  expect(svg(two)).not.toContain('cursor:pointer')
})
