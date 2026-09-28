import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import type { FitLens } from '@/lib/codemap/types'
import type { DiagramTokens } from '@/lib/diagram-theme'
import fixture from './__fixtures__/fit.json'
import { Fit, FitList, type FitProps } from './Fit'

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

const fit = fixture as FitLens
const noop = () => {}
const props = (width: number): FitProps => ({
  fit,
  width,
  popped: false,
  stale: new Set(),
  selected: null,
  tokens: TOKENS,
  slots: new Map(),
  onSelect: noop,
  onHover: noop,
  onContext: noop,
  onExpandBundle: noop,
})

/** The inner html of the `<g data-node=id>` group. */
function node(html: string, id: string): string {
  const at = html.indexOf(`data-node="${id}"`)
  return at < 0 ? '' : html.slice(at, html.indexOf('</g>', at))
}

test('Fit at 800 px: 20 nodes incl. two bundles, 7 dashed edges, no arrowheads, badges on the focus', () => {
  const html = renderToStaticMarkup(createElement(Fit, props(800)))
  expect(html.match(/data-node="/g)?.length).toBe(20)
  expect(html).toContain('data-node="bundle:0"')
  expect(html).toContain('data-node="bundle:2"')
  expect(html.match(/stroke-dasharray="6 4"/g)?.length).toBe(7)
  expect(html).not.toContain('marker-end')
  expect(html).not.toContain('<marker')
  expect(html).toContain('>+14 callers</tspan>')
  expect(node(html, fit.focus.moniker)).toMatch(/impacted · \?3</)
  expect(html.match(/class="codemap-node"/g)?.length).toBe(20)
  expect(html.match(/font-size="11"/g)?.length).toBeGreaterThanOrEqual(1)
})

test('FitList: focus header, callers/callees with counts, +N more rows, edge glyphs', () => {
  const html = renderToStaticMarkup(createElement(FitList, props(400)))
  expect(html).not.toContain('<svg')
  expect(html).toContain('callers (12)')
  expect(html).toContain('callees (7)')
  expect(html).toContain('+14 more')
  expect(html).toContain('+1 more')
  expect(html).toContain('?3')
  expect(html.match(/╍╍╍/g)?.length).toBe(7)
  expect(html.match(/━━━/g)?.length).toBe(12)
  // 73 §6.5: two `tools.rs` callers get distinguishing suffixes, never a middle ellipsis
  expect(html).toContain('scrape/src/tools.rs')
  expect(html).not.toContain('…')
})

test('Fit under 600 px renders the list form', () => {
  expect(renderToStaticMarkup(createElement(Fit, props(599)))).not.toContain('<svg')
})
