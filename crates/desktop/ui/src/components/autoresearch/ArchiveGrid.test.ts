import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import { ArchiveGrid, binsOf, distanceFromChampion } from './ArchiveGrid'
import type { CellRow } from '@/rpc/bindings/CellRow'

const cell = (key: string, n: number, length: number, extra: Partial<CellRow> = {}): CellRow => ({
  key,
  island: Number.parseInt(key.slice(1), 10),
  sha: `${n}`.padStart(8, 'a'),
  n,
  scores: { length },
  descriptor: { time_ms: n * 10 },
  children: 0,
  champion: false,
  ...extra,
})

test('bins of a key: one or two dimensions, none without descriptors', () => {
  expect(binsOf('i0:2.1')).toEqual([2, 1])
  expect(binsOf('i3:4')).toEqual([4])
  expect(binsOf('i1:0')).toEqual([0])
  expect(binsOf('i1:')).toEqual([])
})

test('distance from the champion: 0 at the champion, 1 at the farthest elite, direction-free', () => {
  const d = distanceFromChampion([cell('i0:0', 0, 7450), cell('i0:1', 1, 7000, { champion: true }), cell('i1:1', 2, 7600)])
  expect(d.get('i0:1')).toBe(0)
  expect(d.get('i1:1')).toBe(1)
  expect(d.get('i0:0')).toBeCloseTo(450 / 600)
})

test('grid: one section per island, the champion starred, empty cells dashed, hover text per cell (159 §9)', () => {
  const view = {
    kind: 'archive' as const,
    axes: ['time_ms', ''] as [string, string],
    islands: 2,
    cells: [cell('i0:0', 0, 7450, { children: 1 }), cell('i0:1', 1, 7000, { champion: true }), cell('i1:0', 0, 7450, { children: 1 }), cell('i1:1', 2, 7600)],
  }
  const html = renderToStaticMarkup(createElement(ArchiveGrid, { view }))
  expect(html).toContain('data-kind="archive"')
  expect((html.match(/data-island="/g) ?? []).length).toBe(2)
  expect((html.match(/data-cell="/g) ?? []).length).toBe(4)
  expect((html.match(/data-champion="true"/g) ?? []).length).toBe(1)
  expect(html).toContain('★ #1')
  expect(html).toContain('time_ms 10')
  expect(html).toContain('champion (re-measured at finalize)')
  expect(html).toContain('children 1')
  expect(html).not.toContain('data-empty=')
  // A second descriptor adds rows; cells the archive has not reached are empty.
  const two = { ...view, axes: ['time_ms', 'peak_kb'] as [string, string], cells: [cell('i0:0.0', 0, 7450), cell('i0:1.1', 1, 7000, { champion: true }), cell('i1:0.0', 0, 7450)] }
  const h2 = renderToStaticMarkup(createElement(ArchiveGrid, { view: two }))
  expect((h2.match(/data-empty="/g) ?? []).length).toBe(2 * 4 - 3)
  expect(h2).toContain('peak_kb ↑')
  expect(h2).toContain('time_ms →')
})

test('grid: no elite yet says so', () => {
  const html = renderToStaticMarkup(createElement(ArchiveGrid, { view: { kind: 'archive' as const, axes: ['', ''] as [string, string], islands: 3, cells: [] } }))
  expect(html).toContain('No elite yet')
  expect(html).toContain('data-kind="archive"')
})
