import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import { OpenUITheme, theuthOpenUITheme } from './openui-theme'

/**
 * The skin contract (46 §6): every colour the app renderer can paint comes from a theuth token — a `var()` reference
 * or a `color-mix` wash of one — so no OpenUI default grey can surface, in either mode (the tokens flip with `.dark`)
 * or under a snapshot's forced appearance (its frame re-declares them). The scrim is the one literal, `ui/dialog`'s.
 */
const TOKEN = /^(var\(--|color-mix\(in oklab, var\(--)/

test('every mapped colour is a theuth token reference or a wash of one', () => {
  for (const [key, value] of Object.entries(theuthOpenUITheme)) {
    // Radii and font families carry their own tests below; the palette contract is about colour.
    if (Array.isArray(value) || key.startsWith('radius') || key.startsWith('font')) continue
    if (key === 'overlay') {
      expect(value).toBe('rgb(0 0 0 / 50%)')
      continue
    }
    expect(value, `${key} must map a theuth token`).toMatch(TOKEN)
  }
})

test('the chart series is the shadcn --chart-1..8 order, never ochre or carnelian', () => {
  expect(theuthOpenUITheme.defaultChartPalette).toEqual([
    'var(--chart-1)',
    'var(--chart-2)',
    'var(--chart-3)',
    'var(--chart-4)',
    'var(--chart-5)',
    'var(--chart-6)',
    'var(--chart-7)',
    'var(--chart-8)',
  ])
})

test('the radii follow the design radius, the wells and popovers stay monotone', () => {
  const { radiusS, radiusM, radiusL, radiusXl, radius2xl, radius3xl } = theuthOpenUITheme
  expect([radiusS, radiusM, radiusL, radiusXl, radius2xl, radius3xl]).toEqual([
    'calc(var(--radius) - 4px)',
    'calc(var(--radius) - 2px)',
    'var(--radius)',
    'var(--radius)',
    'calc(var(--radius) + 4px)',
    'calc(var(--radius) + 4px)',
  ])
})

test('OpenUI is set in the bundled variable Inter, so its wght-axis settings render', () => {
  expect(theuthOpenUITheme.fontBody).toBe('"Inter Variable", "Inter", sans-serif')
  expect(theuthOpenUITheme.fontHeading).toBe('"Inter Variable", "Inter", sans-serif')
  expect(theuthOpenUITheme.fontLabel).toBe('"Inter Variable", "Inter", sans-serif')
  expect(theuthOpenUITheme.fontNumbers).toBe('"Inter Variable", "Inter", sans-serif')
})

test('the mount scopes instead of writing the body: a display-contents wrapper carries the scope class', () => {
  const html = renderToStaticMarkup(createElement(OpenUITheme, { mode: 'dark' }, createElement('p', null, 'x')))
  expect(html).toContain('class="openui-theuth-scope"')
  expect(html).toContain('display:contents')
  expect(html).toContain('<p>x</p>')
})
