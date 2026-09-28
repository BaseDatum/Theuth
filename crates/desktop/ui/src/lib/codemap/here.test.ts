import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import { bigHere } from '@/components/codemap/__fixtures__/generate'
import fixture from '@/components/codemap/__fixtures__/here.json'
import { HereList } from '@/components/codemap/Here'
import { applyHereDelta, buildHereRows, focusMatches, type HereInput, type HereRow, MORE_DIRS } from './here'
import type { HereLens, MapRow } from './types'

const here = fixture as HereLens
const none = new Set<string>()
const input = (h: HereLens, o: Partial<HereInput> = {}): HereInput => ({ here: h, expanded: none, collapsed: none, focus: [], edited: [], marker: null, ...o })
const dirsOf = (rows: HereRow[]) => rows.filter((r) => r.kind === 'dir')
const noop = () => {}
const render = (rows: HereRow[]) => renderToStaticMarkup(createElement(HereList, { rows, onHover: noop, onSelect: noop, onContext: noop }))

test('focusMatches: path prefix or exact name', () => {
  const r = here.rows[1]!
  expect(focusMatches(r, ['crates/tools'])).toBe(true)
  expect(focusMatches(r, ['start'])).toBe(true)
  expect(focusMatches(r, ['crates/runtime', 'Start'])).toBe(false)
})

test('dirs < 3 % collapse unless they hold a focus/edited/marker file; overrides win', () => {
  const big = bigHere(2000)
  const cold = buildHereRows(input(big))
  const small = dirsOf(cold).filter((d) => d.kind === 'dir' && !d.coverage && d.ranked < 60)
  expect(small.length).toBeGreaterThan(0)
  expect(small.every((d) => !d.open)).toBe(true)
  expect(dirsOf(cold).some((d) => d.kind === 'dir' && d.ranked >= 60 && d.open)).toBe(true)
  const target = small[0]!.path
  const file = big.rows.find((r) => r.path.startsWith(`${target}/`))!.path
  for (const o of [{ focus: [file] }, { edited: [file] }, { marker: file }, { expanded: new Set([target]) }]) {
    const d = dirsOf(buildHereRows(input(big, o))).find((x) => x.path === target)!
    expect(d.kind === 'dir' && d.open, JSON.stringify(o)).toBe(true)
  }
  const forced = dirsOf(buildHereRows(input(big, { collapsed: new Set(['crates/d0']) }))).find((x) => x.path === 'crates/d0')!
  expect(forced.kind === 'dir' && forced.open).toBe(false)
})

test('≤ 12 symbols per file then a counted more row; expanding more:<file> lifts the cap', () => {
  const big = bigHere(2000)
  const rows = buildHereRows(input(big))
  const i = rows.findIndex((r) => r.kind === 'file' && r.open && r.symbols > 12)
  const f = rows[i]!
  if (f.kind !== 'file') throw new Error('file')
  expect(f.hidden).toBe(f.symbols - 12)
  const syms = rows.slice(i + 1, i + 13)
  expect(syms.every((r) => r.kind === 'symbol')).toBe(true)
  expect(syms.map((r) => r.kind === 'symbol' && r.row.line)).toEqual([...syms.map((r) => r.kind === 'symbol' && r.row.line)].sort((a, b) => +a - +b))
  expect(rows[i + 13]).toMatchObject({ kind: 'more', depth: 2, count: f.hidden, parent: f.path })
  const all = buildHereRows(input(big, { expanded: new Set([`more:${f.path}`]) }))
  const j = all.findIndex((r) => r.key === f.key)
  expect(all[j]).toMatchObject({ hidden: 0 })
  expect(all.slice(j + 1, j + 1 + f.symbols).every((r) => r.kind === 'symbol')).toBe(true)
})

test('coverage dirs appended collapsed with counts; ranked files ordered by best rank', () => {
  const rows = buildHereRows(input(here))
  const cov = dirsOf(rows).filter((d) => d.kind === 'dir' && d.coverage)
  expect(cov.map((d) => d.path)).toEqual(here.dirs.map((d) => d.path))
  expect(cov.every((d) => d.kind === 'dir' && !d.open)).toBe(true)
  expect(cov[0]).toMatchObject({ files: 412, ranked: 0, importers: 0 })
  expect(rows.slice(-cov.length)).toEqual(cov)
  expect(dirsOf(rows)[0]).toMatchObject({ path: 'crates/tools', files: 3, ranked: 7, importers: 14 + 56 + 14, open: true })
  expect(rows.filter((r) => r.kind === 'file').map((r) => r.key).slice(0, 3)).toEqual(['crates/tools/src/registry.rs', 'crates/tools/src/lib.rs', 'crates/tools/src/edit.rs'])
})

test('beyond 12 open dirs: the rest collapse behind `+N directories`; more:dirs lifts it', () => {
  const uniform = bigHere(2000, 1)
  const rows = buildHereRows(input(uniform))
  const ranked = dirsOf(rows).filter((d) => d.kind === 'dir' && !d.coverage)
  expect(ranked).toHaveLength(15)
  expect(ranked.filter((d) => d.kind === 'dir' && d.open)).toHaveLength(12)
  expect(rows.at(-1)).toMatchObject({ kind: 'more', key: MORE_DIRS, depth: 1, count: 3 })
  const lifted = buildHereRows(input(uniform, { expanded: new Set([MORE_DIRS]) }))
  expect(dirsOf(lifted).filter((d) => d.kind === 'dir' && d.open)).toHaveLength(15)
  expect(lifted.some((r) => r.key === MORE_DIRS)).toBe(false)
})

test('marker and focus flags', () => {
  const rows = buildHereRows(input(here, { marker: 'crates/tools/src/registry.rs#L1366', focus: ['crates/tools', 'Runtime'], edited: ['crates/tools/src/lib.rs'] }))
  const marked = rows.filter((r) => (r.kind === 'symbol' || r.kind === 'file') && r.marker)
  expect(marked).toHaveLength(1)
  expect(marked[0]).toMatchObject({ kind: 'symbol', row: { name: 'start', line: 1366 } })
  const reg = rows.find((r) => r.key === 'crates/tools/src/registry.rs')!
  expect(reg).toMatchObject({ kind: 'file', focus: true, marker: false, callers: 9, importers: 14, entry: true, edited: false })
  expect(rows.find((r) => r.key === 'crates/tools/src/lib.rs')).toMatchObject({ edited: true, focus: true })
  expect(rows.find((r) => r.key === 'rust:crates/runtime/src/lib.rs#Runtime(struct)')).toMatchObject({ focus: true })
  expect(rows.find((r) => r.key === 'rust:crates/runtime/src/lib.rs#Runtime::publish(method)')).toMatchObject({ focus: false })
  const bare = buildHereRows(input(here, { marker: 'crates/tools/src/registry.rs' }))
  expect(bare.filter((r) => (r.kind === 'symbol' || r.kind === 'file') && r.marker)).toMatchObject([{ kind: 'file', path: 'crates/tools/src/registry.rs' }])
})

test('applyHereDelta keeps survivors identical, counts changed/added/removed', () => {
  const prev = bigHere(2000).rows
  const next = prev.map((r, i) => (i % 40 === 0 ? { ...r, callers: r.callers + 1 } : r))
  const t0 = performance.now()
  const { rows, changed } = applyHereDelta(prev, next)
  const ms = performance.now() - t0
  console.log(`applyHereDelta 2000 rows / 50 changed: ${ms.toFixed(2)} ms`)
  expect(ms).toBeLessThan(16)
  expect(changed).toBe(50)
  rows.forEach((r, i) => expect(r === prev[i]).toBe(i % 40 !== 0))
  // drop prev[0] (removed), rename next[1] (one add + one removal): 49 edits + 3
  const removed: MapRow[] = next.slice(1)
  removed[0] = { ...removed[0]!, moniker: 'rust:new#x(function)' }
  expect(applyHereDelta(prev, removed).changed).toBe(49 + 3)
})

test('perf: buildHereRows over 2 000 rows < 100 ms; HereList of 40 rows renders', () => {
  const big = bigHere(2000)
  const t0 = performance.now()
  const rows = buildHereRows(input(big))
  const ms = performance.now() - t0
  console.log(`buildHereRows 2000 rows → ${rows.length} tree rows: ${ms.toFixed(2)} ms`)
  expect(ms).toBeLessThan(100)
  const t1 = performance.now()
  const html = render(rows.slice(0, 40))
  console.log(`renderToStaticMarkup HereList × 40 rows: ${(performance.now() - t1).toFixed(2)} ms`)
  expect(html.split('role="button"')).toHaveLength(41)
})

test('render: glyphs, badges, marker, focus bar, coverage count, no zero counts', () => {
  const rows = buildHereRows(input(here, { marker: 'crates/tools/src/registry.rs#L1366', focus: ['crates/tools'] }))
  const html = render(rows)
  for (const s of ['ƒ start', '→14', '↑9', '◆entry', '(412 files)', 'L1366', 'font-mono text-[11px]', 'tabindex="0"']) expect(html).toContain(s)
  expect(html).not.toContain('→0')
  expect(html).not.toContain('↑0')
  const cells = html.split('role="button"')
  const start = cells.find((c) => c.includes('ƒ start'))!
  expect(start).toContain('►')
  expect(start).toContain('▎')
  expect(cells.filter((c) => c.includes('►'))).toHaveLength(1)
  expect(cells.filter((c) => c.includes('▎')).length).toBe(rows.filter((r) => (r.kind === 'symbol' || r.kind === 'file') && r.focus).length)
  expect(cells.find((c) => c.includes('crates/runtime/src/lib.rs'))).not.toContain('▎')
  expect(cells.find((c) => c.includes('registry.rs'))).toContain('title="crates/tools/src/registry.rs"')
  expect(renderToStaticMarkup(createElement(HereList, { rows: rows.slice(0, 2), dim: true, onHover: noop, onSelect: noop, onContext: noop }))).toContain('opacity-50')
})
