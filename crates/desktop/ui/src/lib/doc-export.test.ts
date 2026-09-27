import { expect, test } from 'vitest'
import type { DiagramTokens } from './diagram-theme'
import { exportAssets, mountedDiagrams } from './doc-export'

const DARK: DiagramTokens = {
  background: '#12100e',
  foreground: '#F3EADF',
  primary: '#f3eadf',
  muted: '#2a241c',
  mutedForeground: '#a89880',
  border: '#3a3228',
  card: '#1c1915',
  accent: '#d8763a',
  success: '#7fb59a',
  danger: '#e0705f',
  font: 'ui-sans-serif',
}
// Light foreground equals dark primary's hue family but not its value; background/foreground cross over on purpose.
const LIGHT: DiagramTokens = { ...DARK, background: '#f3eadf', foreground: '#1a1612', primary: '#1a1612', card: '#ffffff', border: '#d4c4a8' }

const svg = (viewBox: string | null, inButton = false) => ({
  outerHTML: viewBox ? `<svg viewBox="${viewBox}"/>` : '<svg/>',
  getAttribute: (name: string) => (name === 'viewBox' ? viewBox : null),
  closest: (sel: string) => (sel === 'button' && inButton ? {} : null),
})
const frame = (id: string, svgs: ReturnType<typeof svg>[]) => ({
  getAttribute: (name: string) => (name === 'data-block' ? id : null),
  querySelectorAll: () => svgs,
})

test('mountedDiagrams keys the diagram svg by block id, skipping button icons and frames without a diagram', () => {
  const root = {
    // g1: the Tour button's icon comes first in DOM order; the diagram is the second svg.
    querySelectorAll: () => [frame('g1', [svg('0 0 24 24', true), svg('0 0 10 10')]), frame('s2', []), frame('s3', [svg(null), svg('0 0 4 4')])],
  }
  expect(mountedDiagrams(root)).toEqual([
    { id: 'g1', markup: '<svg viewBox="0 0 10 10"/>' },
    { id: 's3', markup: '<svg viewBox="0 0 4 4"/>' },
  ])
})

test('exportAssets recolours in one pass to the light palette and makes each svg standalone', () => {
  const markup = '<svg viewBox="0 0 20 10"><rect fill="#12100e" stroke="#f3eadf"/><text fill="#F3EADF">a</text><path stroke="#d8763a"/></svg>'
  const asset = exportAssets([{ id: 'g1', markup }], DARK, LIGHT)[0]!
  expect(asset.id).toBe('g1')
  // background → light background, foreground → light foreground, and the light background is not re-swapped.
  expect(asset.svg).toContain('<rect fill="#f3eadf" stroke="#1a1612"/>')
  expect(asset.svg).toContain('<text fill="#1a1612">a</text>')
  // Unchanged tokens stay.
  expect(asset.svg).toContain('stroke="#d8763a"')
  expect(asset.svg).toMatch(/^<svg xmlns="http:\/\/www.w3.org\/2000\/svg" viewBox="0 0 20 10" width="20" height="10"><rect x="-1" y="-1" width="22" height="12" fill="#fff"\/>/)
})

test('exportAssets drops an svg without a viewBox and is identity when the palettes match', () => {
  expect(exportAssets([{ id: 'x', markup: '<svg/>' }], DARK, LIGHT)).toEqual([])
  const a = exportAssets([{ id: 'y', markup: '<svg viewBox="0 0 1 1"><g fill="#12100e"/></svg>' }], DARK, DARK)[0]!
  expect(a.svg).toContain('fill="#12100e"')
})
