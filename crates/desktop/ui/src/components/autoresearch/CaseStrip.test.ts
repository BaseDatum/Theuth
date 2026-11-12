import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import type { CaseRow } from "@/rpc/bindings/method.autoresearch.output"
import { CaseStrip, experimentNumbers, splitMeans } from './CaseStrip'

const rows: CaseRow[] = [
  { id: 'v1', split: 'val', values: { 0: 0.0, 1: 1.0, 4: 1.0 } },
  { id: 'v2', split: 'val', values: { 0: 1.0, 1: 1.0, 4: 1.0 } },
  { id: 'h1', split: 'holdout', values: { 0: 0.0, 4: 1.0 } },
  { id: 't1', split: 'train', values: { 0: 0.5, 1: 0.5 } },
]
const count = (html: string, re: RegExp) => (html.match(re) ?? []).length

test('case strip (159 W2 §7): val first, then holdout, then train; one cell per case × column, blank where unscored; both staircases', () => {
  expect(experimentNumbers(rows)).toEqual([0, 1, 4])
  expect(splitMeans(rows, 'val', [0, 1, 4])).toEqual([0.5, 1.0, 1.0])
  expect(splitMeans(rows, 'holdout', [0, 1, 4])).toEqual([0.0, null, 1.0])
  const html = renderToStaticMarkup(createElement(CaseStrip, { view: { kind: 'cases', rows, split: { train: 1, val: 2, holdout: 1 } } }))
  expect(html).toContain('data-kind="cases"')
  expect(html.indexOf('data-split="val"')).toBeLessThan(html.indexOf('data-split="holdout"'))
  expect(html.indexOf('data-split="holdout"')).toBeLessThan(html.indexOf('data-split="train"'))
  // 4 cases × 3 columns = 12 cells; h1 at #1 and t1 at #4 are unscored and have no title.
  expect(count(html, /data-case="/g)).toBe(12)
  expect(count(html, /<title>/g)).toBe(10)
  expect(html).toContain('v1 · base · 0.00')
  expect(html).toContain('h1 · #4 · 1.00')
  // The holdout stair skips column #1 (no holdout at #1): two points; the val stair has three.
  const val = /data-series="val"/.exec(html)
  expect(val).not.toBeNull()
  expect(html).toMatch(/d="M[\d.]+ [\d.]+ H[\d.]+ V[\d.]+ H[\d.]+ V[\d.]+" fill="none" stroke="var\(--status-success-fg\)"/)
  expect(html).toMatch(/d="M[\d.]+ [\d.]+ H[\d.]+ V[\d.]+" fill="none" stroke="var\(--fg-accent\)"/)
  expect(html).toContain('1 train · 2 val · 1 holdout')
  expect(html).toContain('val mean 1.00')
})

test('case strip: nothing scored yet paints the waiting card', () => {
  const html = renderToStaticMarkup(createElement(CaseStrip, { view: { kind: 'cases', rows: [], split: { train: 0, val: 0, holdout: 0 } } }))
  expect(html).toContain('baseline is being measured')
})
