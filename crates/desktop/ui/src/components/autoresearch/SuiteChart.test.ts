import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import { SuiteChart } from './SuiteChart'

const count = (html: string, re: RegExp) => (html.match(re) ?? []).length

test('suite chart: two bars per fixture, held-out group behind a divider, a missing side draws no bar, regressions hang below the base line (159 §12)', () => {
  const html = renderToStaticMarkup(
    createElement(SuiteChart, {
      view: {
        kind: 'suite',
        fixtures: [
          { fixture: 'toy-sort', held_out: false, incumbent: 0.55, candidate: 0.67 },
          { fixture: 'toy-dedupe', held_out: false, incumbent: 0.4, candidate: -0.2 },
          { fixture: 'toy-lru', held_out: true, incumbent: 0.5, candidate: null },
        ],
      },
    }),
  )
  expect(html).toContain('data-kind="suite"')
  expect(count(html, /data-held-out="false"/g)).toBe(2)
  expect(count(html, /data-held-out="true"/g)).toBe(1)
  expect(count(html, /data-role="incumbent"/g)).toBe(3)
  expect(count(html, /data-role="candidate"/g)).toBe(2)
  expect(html).toContain('held out')
  expect(html).toContain('0.40 → -0.20')
  expect(html).toContain('0.50 → —')
  // The regression's bar starts at the base line and extends down: its y equals y(0).
  const base = /<line[^>]*y1="([\d.]+)"[^>]*stroke-opacity="0.35"/.exec(html)?.[1]
  const neg = /data-fixture="toy-dedupe".*?data-role="candidate"/s.exec(html)?.[0]
  expect(base).toBeDefined()
  expect(neg).toContain(`y="${base}"`)
})

test('suite chart: an empty suite says so instead of drawing axes', () => {
  const html = renderToStaticMarkup(createElement(SuiteChart, { view: { kind: 'suite', fixtures: [] } }))
  expect(html).toContain('No fixtures in the suite yet.')
  expect(html).not.toContain('<svg')
})
