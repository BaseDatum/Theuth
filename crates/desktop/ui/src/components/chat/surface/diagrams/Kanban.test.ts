import { readFileSync } from 'node:fs'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { JsonRenderFence } from '../JsonRenderFence'
import { parseSpec } from '../parse'
import { Kanban, type KanbanData, kanbanUnits } from './Kanban'

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { message: vi.fn(), error: vi.fn() }) }))
vi.mock('@/lib/rpc', () => ({ useApi: () => ({}) }))
vi.mock('@/components/ui/tooltip', () => ({ Tooltip: ({ children }: { children: unknown }) => children, TooltipTrigger: ({ children }: { children: unknown }) => children, TooltipContent: () => null }))
vi.mock('@/lib/theme', () => ({ subscribeDark: () => () => {} }))

const RUST = new URL('../../../../../../../surface/fixtures/json-render/', import.meta.url)
const load = (name: string) => {
  const code = readFileSync(new URL(`${name}.json`, RUST), 'utf8')
  const s = parseSpec(code)
  if ('error' in s) throw new Error(s.error)
  return { code, data: Object.values(s.spec.elements).find((e) => e.type === 'Kanban')!.props as unknown as KanbanData }
}
const rows = load('valid-kanban-rows')
const plain = load('valid-kanban')
const html = (d: KanbanData, extra: Record<string, unknown> = {}) => renderToStaticMarkup(createElement(Kanban, { props: d, ...extra }))

test('story map: three labelled bands, the release cut after Should, n/limit headers, state glyphs, unit buttons', () => {
  const h = html(rows.data)
  expect(h).toMatchSnapshot()
  expect(h).toContain('>Story map</div>')
  const bands = [...h.matchAll(/data-band="([^"]*)"/g)].map((m) => m[1])
  expect(bands).toEqual(['Must', 'Should', 'Could'])
  expect(h.match(/data-cut/g)).toHaveLength(1)
  const cut = h.indexOf('data-cut')
  expect(cut).toBeGreaterThan(h.indexOf('data-band="Should"'))
  expect(cut).toBeLessThan(h.indexOf('data-band="Could"'))
  expect(h).toMatch(/role="separator" aria-label="release cut"/)
  // both limited columns hold 3 cards over a limit of 2 → `3/2 ⚠`; Boards has no limit → a bare count
  expect(h.match(/>3\/2</g)).toHaveLength(2)
  expect(h.match(/aria-label="over limit"/g)).toHaveLength(2)
  expect(h).toContain('>1</span>')
  expect(h.match(/⊘/g)).toHaveLength(1)
  expect(h.match(/✓/g)).toHaveLength(3)
  expect(h).toContain('aria-label="Draw a chart, blocked"')
  expect(h).toContain('aria-description="crates/surface/src/render.rs"')
  expect(h).toContain('aria-label="Install, done"')
  expect(h.match(/<div data-unit="[^"]+" role="button" tabindex="0"/g)).toHaveLength(7)
  expect(kanbanUnits(rows.data)).toEqual(['install', 'login', 'tour', 'ask', 'draw', 'export', 'board'])
  expect(h).toContain('grid-template-columns:repeat(3, minmax(12rem, 1fr))')
})

test('over limit: n/limit turns to 2/1 with an ⚠ marker', () => {
  const h = html({ columns: [{ title: 'Doing', limit: 1, cards: [{ title: 'a' }, { title: 'b' }] }] })
  expect(h).toContain('>2/1<')
  expect(h).toContain('aria-label="over limit"')
  expect(h).toContain('⚠')
  expect(kanbanUnits({ columns: [{ title: 'Doing', cards: [{ title: 'a' }, { title: 'b', id: 'x' }] }] })).toEqual(['0:0', 'x'])
})

test('highlight paints only the highlighted card; onSelect leaves the others at full opacity', () => {
  const h = html(rows.data, { highlight: ['draw'], onSelect: () => {} })
  const draw = /<div data-unit="draw"[^>]*>/.exec(h)?.[0] ?? ''
  expect(draw).toContain('outline:2px solid var(--fg-accent)')
  const tour = /<div data-unit="tour"[^>]*>/.exec(h)?.[0] ?? ''
  expect(tour).not.toContain('opacity-50')
  expect(tour).not.toContain('outline')
})

test('streaming tolerance: a row past rows.length lands in the trailing unlabelled band without throwing', () => {
  const d: KanbanData = { rows: ['A'], columns: [{ title: 'c', cards: [{ title: 'late', row: 9 }, { title: 'early', row: 0 }] }] }
  const h = html(d)
  const bands = [...h.matchAll(/data-band="([^"]*)"/g)].map((m) => m[1])
  expect(bands).toEqual(['A', ''])
  expect(h.indexOf('>late<')).toBeGreaterThan(h.indexOf('data-band=""'))
  expect(html({ columns: [] })).toContain('<div')
})

test('the plain board keeps its column layout: counts without a slash, no bands', () => {
  const h = html(plain.data)
  expect(h).toContain('rounded-xl bg-layer-2')
  expect(h).not.toMatch(/>\d+\/\d+</)
  expect(h).not.toContain('data-band')
  expect(h.match(/data-unit="\d+:\d+"/g)).toHaveLength(4)
})

test('mounts through the fence without the stub marker', () => {
  const h = renderToStaticMarkup(createElement(JsonRenderFence, { code: rows.code, isIncomplete: false, language: 'json-render' }))
  expect(h).not.toContain('data-stub')
  expect(h).toContain('data-streamdown="json-render"')
  expect(h).toContain('data-cut')
})
