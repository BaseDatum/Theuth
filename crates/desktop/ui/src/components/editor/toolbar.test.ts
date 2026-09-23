import { expect, test } from 'vitest'
import { keyboardInset } from '@/lib/mobile'
import { BLOCK_TYPES, blockTypeKey, nextVisibility, toolbarVisible } from './toolbar'

test('keyboard inset is the band below the visual viewport, never negative, absent without the API', () => {
  // iPhone 14: 844 layout, 336 keyboard; WebKit shrinks only the visual viewport.
  expect(keyboardInset(844, { height: 508, offsetTop: 0 })).toBe(336)
  // WebKit scrolled the page under the keyboard: the offset is part of the hidden band.
  expect(keyboardInset(844, { height: 508, offsetTop: 100 })).toBe(236)
  // `interactive-widget=resizes-content`: layout shrinks too — a fixed bar needs no inset.
  expect(keyboardInset(508, { height: 508, offsetTop: 0 })).toBe(0)
  // Rounding noise and a pinch-zoomed viewport taller than the layout clamp to 0.
  expect(keyboardInset(844, { height: 844.4, offsetTop: 0 })).toBe(0)
  expect(keyboardInset(844, null)).toBe(0)
})

test('toolbar visibility: focus shows, blur waits out the grace, readonly hides, a sheet keeps it', () => {
  let v = nextVisibility('hidden', 'focus')
  expect(v).toBe('shown')
  v = nextVisibility(v, 'blur')
  expect(v).toBe('grace')
  expect(toolbarVisible(v, false, false)).toBe(true)
  // Focus back within the grace (a toolbar tap): shown again; a late `elapsed` for the old blur changes nothing.
  v = nextVisibility(v, 'focus')
  expect(nextVisibility(v, 'elapsed')).toBe('shown')
  v = nextVisibility(nextVisibility(v, 'blur'), 'elapsed')
  expect(v).toBe('hidden')
  expect(toolbarVisible(v, false, false)).toBe(false)
  // A blur while hidden stays hidden (no phantom grace).
  expect(nextVisibility('hidden', 'blur')).toBe('hidden')
  // Read-only wins over everything; an open sheet keeps the bar through the blur it caused.
  expect(toolbarVisible('shown', true, true)).toBe(false)
  expect(toolbarVisible('hidden', false, true)).toBe(true)
})

test('block-type mapping: the §10 rows in order; headings by level; mermaid code is Diagram; other blocks map to nothing', () => {
  expect(BLOCK_TYPES.map((t) => t.key)).toEqual(['paragraph', 'h1', 'h2', 'h3', 'bullet', 'numbered', 'check', 'quote', 'code', 'diagram'])
  expect(blockTypeKey({ type: 'paragraph' })).toBe('paragraph')
  expect(blockTypeKey({ type: 'heading', props: { level: 2 } })).toBe('h2')
  expect(blockTypeKey({ type: 'heading', props: { level: 4 } })).toBeNull()
  expect(blockTypeKey({ type: 'bulletListItem' })).toBe('bullet')
  expect(blockTypeKey({ type: 'checkListItem', props: { checked: true } })).toBe('check')
  expect(blockTypeKey({ type: 'codeBlock', props: { language: 'ts' } })).toBe('code')
  expect(blockTypeKey({ type: 'codeBlock', props: { language: 'mermaid' } })).toBe('diagram')
  for (const type of ['table', 'divider', 'frontmatter', 'toggleListItem']) expect(blockTypeKey({ type })).toBeNull()
})
