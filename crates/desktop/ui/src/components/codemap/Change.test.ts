import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import type { ChangeLens } from '@/lib/codemap/types'
import type { DiagramTokens } from '@/lib/diagram-theme'
import fixture from './__fixtures__/change.json'
import { Change, ChangeList, type ChangeProps, litChanged, modeChip } from './Change'

const TOKENS: DiagramTokens = {
  background: '#e8dcc8',
  foreground: '#1a1612',
  primary: '#1a1612',
  muted: '#d4c4a8',
  mutedForeground: '#6b5e4f',
  border: '#d4c4a8',
  card: '#f3eadf',
  accent: '#1d5f82',
  success: '#265c47',
  danger: '#8f2f1f',
  font: 'sans-serif',
  series: ['#1d5f82', '#265c47', '#3b5b8c', '#8a6d1f', '#7a3e7a', '#6b5e4f'],
}

const change = fixture as unknown as ChangeLens
const noop = () => {}
const props = (width: number, selected: string | null = null): ChangeProps => ({
  change,
  width,
  popped: false,
  selected,
  tokens: TOKENS,
  slots: new Map(),
  onSelect: noop,
  onHover: noop,
  onContext: noop,
  onExpandBundle: noop,
})

const GROUPS = ['changed (3)', 'direct (12)', 'transitive (4)', 'tests (8)']

test('Change at 800 px: four groups, no edges at rest, stale hatch, chip and footer', () => {
  const html = renderToStaticMarkup(createElement(Change, props(800)))
  const svgs = html.split('<svg')
  expect(svgs.length).toBe(2)
  for (const g of GROUPS) expect(html).toContain(`>${g}</text>`)
  expect(svgs[1]).not.toContain('<path')
  expect(html).not.toContain('marker-end')
  const lens = html.indexOf('data-node="crates/codeindex/src/lens.rs"')
  expect(html.slice(lens, html.indexOf('</g>', lens))).toContain('fill="url(#codemap-hatch)"')
  expect(html.slice(lens, html.indexOf('</g>', lens))).toContain('stale</text>')
  expect(html).toContain('<pattern id="codemap-hatch"')
  expect(html).toContain('worktree · since 22b3945')
  expect(html).toContain('464 ambiguous skipped · 2 unresolved imports')
  expect(html).toContain('>changed</text>')
  expect(html).toContain('>impacted</text>')
})

test('selecting a direct file draws one accent line per lit changed file in the overlay', () => {
  expect(litChanged(change, 'crates/rpc/src/api.rs')).toEqual(['crates/codeindex/src/index.rs'])
  expect(litChanged(change, 'crates/codeindex/src/outline_hook.rs')).toEqual(change.changed.map((f) => f.path))
  expect(litChanged(change, 'crates/codeindex/src/index.rs')).toEqual([])
  const html = renderToStaticMarkup(createElement(Change, props(800, 'crates/codeindex/src/outline_hook.rs')))
  const overlay = html.split('<svg')[2] ?? ''
  expect(overlay).toContain(`stroke="${TOKENS.accent}" stroke-width="2"`)
  expect(overlay.match(/<line /g)?.length).toBe(3)
  expect(html.split('<svg')[1]).not.toContain('<path')
})

test('modeChip per mode', () => {
  expect(modeChip(change)).toBe('worktree · since 22b3945')
  expect(modeChip({ ...change, mode: 'range', range: ['main', 'HEAD'] })).toBe('main..HEAD')
  expect(modeChip({ ...change, mode: 'files' })).toBe('3 files')
})

test('ChangeList: four sections with counts, glyphs, badges, footer', () => {
  const html = renderToStaticMarkup(createElement(ChangeList, props(400)))
  expect(html).not.toContain('<svg')
  for (const g of GROUPS) expect(html).toContain(g)
  expect(html.match(/>~</g)?.length).toBeGreaterThanOrEqual(2)
  expect(html).toContain('>✓<')
  expect(html).toContain('via Stats, reconcile')
  expect(html.match(/>changed</g)?.length).toBe(3)
  expect(html.match(/>test</g)?.length).toBe(8)
  expect(html).toContain('>stale<')
  expect(html).toContain('464 ambiguous skipped · 2 unresolved imports')
  expect(renderToStaticMarkup(createElement(Change, props(599)))).not.toContain('<svg')
})
