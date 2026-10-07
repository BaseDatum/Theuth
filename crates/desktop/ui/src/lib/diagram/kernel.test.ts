import { expect, test } from 'vitest'
import { countOf, describe, svgA11y } from './a11y'
import { flattenElk, fnv, wrapLabel } from './elk'
import { truncate } from './label'
import { markers, orthogonalPath, pointAt } from './path'
import { band, floorTo, formatTick, linear, niceDomain, parseIso, ticks, time, timeUnit } from './scale'
import { paintOf, UNITS_IDLE, unitsReduce } from './units'

test('linear maps, inverts and ticks at 1-2-5 steps', () => {
  const s = linear([0, 100], [0, 200])
  expect(s(25)).toBe(50)
  expect(s.invert(150)).toBe(75)
  expect(s.ticks(5)).toEqual([0, 20, 40, 60, 80, 100])
  expect(ticks(0, 7, 5)).toEqual([0, 1, 2, 3, 4, 5, 6, 7])
  expect(ticks(0, 7, 3)).toEqual([0, 2, 4, 6])
  expect(ticks(0.1, 0.9, 4)).toEqual([0.2, 0.4, 0.6, 0.8])
  expect(ticks(-12, 8, 5)).toEqual([-10, -5, 0, 5])
  expect(niceDomain(3, 97)).toEqual([0, 100])
  expect(niceDomain(5, 5)).toEqual([4, 6])
  // A zero-width domain never divides by zero.
  const flat = linear([4, 4], [0, 100])
  expect(flat(4)).toBe(0)
  expect(flat.invert(50)).toBe(4)
})

test('band spreads categories with padding and reports bandwidth', () => {
  const b = band(['a', 'b', 'c'], [0, 320], 0.2)
  expect(b.step).toBeCloseTo(100)
  expect(b.bandwidth).toBeCloseTo(80)
  expect(b('a')).toBeCloseTo(20)
  expect(b('c')).toBeCloseTo(220)
  expect(b('zz')).toBeCloseTo(20)
})

test('time picks the unit by span and snaps ticks to UTC boundaries (weeks start Monday)', () => {
  expect(timeUnit(3 * 3_600_000)).toBe('hour')
  expect(timeUnit(6 * 86_400_000)).toBe('day')
  expect(timeUnit(40 * 86_400_000)).toBe('week')
  expect(timeUnit(300 * 86_400_000)).toBe('month')
  const s = time([new Date('2026-10-07T00:00Z'), new Date('2026-10-12T00:00Z')], [0, 500])
  expect(s(new Date('2026-10-08T00:00Z'))).toBe(100)
  expect(s.ticks().map((d) => d.toISOString().slice(0, 10))).toEqual(['2026-10-07', '2026-10-08', '2026-10-09', '2026-10-10', '2026-10-11', '2026-10-12'])
  expect(floorTo(new Date('2026-10-07T10:00Z'), 'week').toISOString()).toBe('2026-10-05T00:00:00.000Z')
  expect(formatTick(new Date('2026-10-07T09:05Z'), 'hour')).toBe('09:05')
  expect(formatTick(new Date('2026-10-07T00:00Z'), 'day')).toBe('Oct 7')
  expect(formatTick(new Date('2026-10-01T00:00Z'), 'month')).toBe('Oct 2026')
})

test('parseIso mirrors crates/surface::iso_datetime', () => {
  expect(parseIso('2026-10-07')?.toISOString()).toBe('2026-10-07T00:00:00.000Z')
  expect(parseIso('2026-10-07T09:12:30.250+02:00')?.toISOString()).toBe('2026-10-07T07:12:30.250Z')
  expect(parseIso('2024-02-29')).not.toBeNull()
  for (const bad of ['Oct 7', '2026-13-01', '2023-02-29', '2026-10-7', '2026-10-07T24:00']) expect(parseIso(bad), bad).toBeNull()
})

test('orthogonalPath rounds interior elbows with r=8 and leaves straight runs straight', () => {
  expect(orthogonalPath([{ x: 0, y: 0 }, { x: 100, y: 0 }])).toBe('M0 0 L100 0')
  const d = orthogonalPath([{ x: 0, y: 0 }, { x: 50, y: 0 }, { x: 50, y: 40 }])
  expect(d).toBe('M0 0 L42 0 Q50 0 50 8 L50 40')
  // Short segments shrink the radius so the curve never overshoots.
  expect(orthogonalPath([{ x: 0, y: 0 }, { x: 4, y: 0 }, { x: 4, y: 4 }])).toBe('M0 0 L2 0 Q4 0 4 2 L4 4')
  expect(markers('both', 'm')).toEqual({ markerStart: 'url(#m)', markerEnd: 'url(#m)' })
  expect(markers('none', 'm')).toEqual({})
  expect(markers(undefined, 'm')).toEqual({ markerEnd: 'url(#m)' })
  expect(pointAt([{ x: 0, y: 0 }, { x: 10, y: 0 }, { x: 10, y: 10 }], 0.5)).toEqual({ x: 10, y: 0 })
})

test('units: select toggles, Esc clears, selection dims the non-incident, own danger/done win', () => {
  let s = unitsReduce(UNITS_IDLE, { type: 'select', id: 'a' })
  expect(s.selected).toBe('a')
  expect(paintOf('a', null, s, ['b'])).toBe('active')
  expect(paintOf('b', null, s, ['b'])).toBe('active')
  expect(paintOf('c', null, s, ['b'])).toBe('muted')
  expect(paintOf('c', 'danger', s, ['b'])).toBe('muted')
  expect(paintOf('b', 'danger', s, ['b'])).toBe('danger')
  s = unitsReduce(s, { type: 'select', id: 'a' })
  expect(s.selected).toBeNull()
  expect(paintOf('c', null, s, [], ['c'])).toBe('active')
  expect(paintOf('c', 'done', s, [], ['c'])).toBe('done')
  expect(paintOf('c', 'muted', s, [])).toBe('muted')
  s = unitsReduce(s, { type: 'hover', id: 'x' })
  expect(s.hover).toBe('x')
  expect(unitsReduce(s, { type: 'clear' })).toEqual(UNITS_IDLE)
  expect(unitsReduce(UNITS_IDLE, { type: 'clear' })).toBe(UNITS_IDLE)
})

test('a11y: role by interactivity, ids off the base, counts singularise', () => {
  expect(svgA11y('x', false)).toEqual({ svg: { role: 'img', 'aria-labelledby': 'x-title x-desc' }, titleId: 'x-title', descId: 'x-desc' })
  expect(svgA11y('x', true).svg.role).toBe('group')
  expect(countOf(6, 'tasks')).toBe('6 tasks')
  expect(countOf(1, 'tasks')).toBe('1 task')
  expect(countOf(1, 'entities')).toBe('1 entity')
  expect(describe('Gantt', 'Wave 123', 6, 'tasks')).toBe('Gantt: Wave 123 — 6 tasks')
  expect(describe('Gantt', null, 6, 'tasks')).toBe('Gantt: 6 tasks')
})

test('truncate keeps ≤ 14 chars and ellipsises longer text; wrapLabel and fnv are stable', () => {
  expect(truncate('reads')).toBe('reads')
  expect(truncate('a very long edge label')).toBe('a very long e…')
  expect(wrapLabel('one two three four five six seven')).toEqual(['one two three four five', 'six seven'])
  expect(fnv('abc')).toBe(fnv('abc'))
  expect(fnv('abc')).not.toBe(fnv('abd'))
})

test('flattenElk shifts nested children and container-relative edges into root coordinates', () => {
  const l = flattenElk({
    id: 'root',
    width: 300,
    height: 200,
    children: [
      {
        id: 'g:outer',
        x: 10,
        y: 10,
        width: 200,
        height: 150,
        children: [
          { id: 'g:inner', x: 5, y: 20, width: 100, height: 80, children: [{ id: 'n:a', x: 10, y: 10, width: 40, height: 20 }] },
          { id: 'n:b', x: 120, y: 30, width: 40, height: 20 },
        ],
        edges: [{ id: 'e:0', sources: ['n:a'], targets: ['n:b'], container: 'g:outer', sections: [{ id: 's', startPoint: { x: 55, y: 40 }, endPoint: { x: 120, y: 40 }, bendPoints: [{ x: 90, y: 40 }] }] } as never],
      },
    ],
    edges: [{ id: 'e:1', sources: ['n:b'], targets: ['n:a'], sections: [{ id: 't', startPoint: { x: 0, y: 0 }, endPoint: { x: 1, y: 1 } }] }],
  })
  expect(l.groups.map((g) => [g.id, g.x, g.y])).toEqual([
    ['outer', 10, 10],
    ['inner', 15, 30],
  ])
  expect(l.nodes.map((n) => [n.id, n.x, n.y])).toEqual([
    ['a', 25, 40],
    ['b', 130, 40],
  ])
  const e0 = l.edges.find((e) => e.index === 0)!
  expect(e0.sections[0]).toEqual({ start: { x: 65, y: 50 }, bends: [{ x: 100, y: 50 }], end: { x: 130, y: 50 } })
  expect(l.edges.find((e) => e.index === 1)!.sections[0]!.start).toEqual({ x: 0, y: 0 })
})
