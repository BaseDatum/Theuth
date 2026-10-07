import { readFileSync } from 'node:fs'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { JsonRenderFence } from '../JsonRenderFence'

// 123 step 0: every new component and every extension layout mounts through the real path (parse → registry →
// markup) and yields an accessible svg (`role="img"`, `<title>` first, `<desc>` = the TUI's one-line summary) inside
// a `DiagramFrame`. Owning waves replace the stub bodies; this test keeps passing on the a11y contract alone.
vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { message: vi.fn(), error: vi.fn() }) }))
vi.mock('@/lib/rpc', () => ({ useApi: () => ({}) }))
vi.mock('@/components/ui/tooltip', () => ({ Tooltip: ({ children }: { children: unknown }) => children, TooltipTrigger: ({ children }: { children: unknown }) => children, TooltipContent: () => null }))
vi.mock('@/lib/theme', () => ({ subscribeDark: () => () => {} }))
vi.mock('@/lib/diagram-theme', () => ({
  diagramTokens: () => ({
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
  }),
}))

const RUST = new URL('../../../../../../../surface/fixtures/json-render/', import.meta.url)
const fence = (name: string) => renderToStaticMarkup(createElement(JsonRenderFence, { code: readFileSync(new URL(`${name}.json`, RUST), 'utf8'), isIncomplete: false, language: 'json-render' }))

/** `[fixture, component, summary as the Rust placeholder arm prints it]`. */
const STUBS: [string, string, string][] = [
  ['valid-state-machine', 'StateMachine', '6 states'],
  ['valid-entities', 'Entities', '4 entities'],
  ['valid-swimlane', 'Swimlane', '6 steps'],
  ['valid-graph-delta', 'GraphDelta', '4 nodes'],
  ['valid-bar-chart', 'BarChart', '2 series'],
  ['valid-line-chart', 'LineChart', '2 series'],
  ['valid-scatter-chart', 'ScatterChart', '5 points'],
  ['valid-radar-chart', 'RadarChart', '2 series'],
  ['valid-waterfall', 'Waterfall', '6 steps'],
  ['valid-funnel', 'Funnel', '4 stages'],
  ['valid-gantt', 'Gantt', '6 tasks'],
  ['valid-quadrant', 'Quadrant', '5 items'],
  ['valid-treemap', 'Treemap', '6 items'],
  ['valid-sankey', 'Sankey', '4 flows'],
  ['valid-venn', 'Venn', '3 sets'],
  ['valid-loop', 'Loop', '5 steps'],
  ['valid-layers', 'Layers', '4 layers'],
  ['valid-journey', 'Journey', '4 stages'],
  ['valid-fishbone', 'Fishbone', '4 causes'],
  ['valid-timeline-time', 'Timeline', '4 items'],
]

test.each(STUBS)('%s renders the %s stub as an accessible svg in a DiagramFrame', (fixture, name, summary) => {
  const html = fence(fixture)
  expect(html).toContain(`data-stub="${name}"`)
  const svg = /<svg[^>]*data-stub="[^"]+"[^>]*>(.*?)<\/svg>/s.exec(html)
  expect(svg, 'one stub svg').not.toBeNull()
  expect(svg![0]).toMatch(/role="img"/)
  expect(svg![0]).toMatch(/aria-labelledby="[^"]+-title [^"]+-desc"/)
  expect(svg![1]).toMatch(/^<title id="[^"]+-title">/)
  expect(svg![1]).toContain(`— ${summary}</desc>`)
  expect(svg![1]).toContain(`${name} · ${summary}`)
  expect(html).toContain(`data-streamdown="${name.toLowerCase()}-block"`)
})

test('Tree dispatches on layout: list stays the HTML rows, nodes/org/nested mount the TreeDiagram stub', () => {
  const html = fence('valid-tree-layouts')
  expect(html.match(/data-stub="Tree"/g)).toHaveLength(3)
  const list = renderToStaticMarkup(
    createElement(JsonRenderFence, {
      code: JSON.stringify({ root: 't', elements: { t: { type: 'Tree', props: { items: [{ label: 'a' }, { label: 'b', depth: 1 }], layout: 'list' } } } }),
      isIncomplete: false,
      language: 'json-render',
    }),
  )
  expect(list).not.toContain('data-stub')
  expect(list).toContain('>a<')
})

test('Kanban and Heatmap keep their HTML forms from their new files', () => {
  expect(fence('valid-kanban-rows')).toMatch(/<table|rounded-xl bg-layer-2/)
  expect(fence('valid-heatmap-diverging')).toContain('color-mix(in oklab, var(--color-fg-accent)')
  expect(fence('valid-kanban-rows')).not.toContain('data-stub')
})
