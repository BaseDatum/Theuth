/** 73 §6.2 — the Here tree, pure: rows → dir / file / symbol / more rows for the virtualised list. */
import type { HereLens, MapRow } from './types'

export type HereRow =
  | { kind: 'dir'; key: string; path: string; depth: 0; open: boolean; files: number; ranked: number; importers: number; coverage: boolean }
  | {
      kind: 'file'
      key: string
      path: string
      depth: 1
      open: boolean
      importers: number
      callers: number
      recent: boolean
      entry: boolean
      isTest: boolean
      edited: boolean
      focus: boolean
      marker: boolean
      symbols: number
      hidden: number
    }
  | { kind: 'symbol'; key: string; row: MapRow; depth: 2; focus: boolean; marker: boolean }
  | { kind: 'more'; key: string; depth: 1 | 2; count: number; parent: string }

export type HereInput = {
  here: HereLens
  expanded: ReadonlySet<string>
  collapsed: ReadonlySet<string>
  focus: readonly string[]
  edited: readonly string[]
  marker: string | null
}

const SYMBOL_CAP = 12
const OPEN_DIRS = 12
const OPEN_SHARE = 0.03
/** `expanded` key that lifts the directory cap. */
export const MORE_DIRS = 'more:dirs'

/** First two segments of the dirname (`crates/tools`, `src/components`); `src/x.ts` → `src`; `x.rs` → `.`. */
function dirOf(path: string): string {
  const a = path.indexOf('/')
  if (a < 0) return '.'
  const b = path.indexOf('/', a + 1)
  return b < 0 ? path.slice(0, a) : path.slice(0, b)
}

export function focusMatches(row: MapRow, focus: readonly string[]): boolean {
  return focus.some((f) => row.path.startsWith(f) || row.name === f)
}

type File = { path: string; rows: MapRow[]; best: number }
type Dir = { path: string; files: File[]; best: number; n: number }

export function buildHereRows({ here, expanded, collapsed, focus, edited, marker }: HereInput): HereRow[] {
  const editedSet = new Set(edited)
  const hash = marker?.indexOf('#L') ?? -1
  const mPath = marker == null ? null : hash < 0 ? marker : marker.slice(0, hash)
  const mLine = marker != null && hash >= 0 ? Number(marker.slice(hash + 2)) : null
  const mDir = mPath == null ? null : dirOf(mPath)

  const dirs = new Map<string, Dir>()
  const files = new Map<string, File>()
  for (const r of here.rows) {
    let f = files.get(r.path)
    const dp = dirOf(r.path)
    let d = dirs.get(dp)
    if (!d) dirs.set(dp, (d = { path: dp, files: [], best: r.rank, n: 0 }))
    if (!f) {
      files.set(r.path, (f = { path: r.path, rows: [], best: r.rank }))
      d.files.push(f)
    }
    f.rows.push(r)
    d.n++
    if (r.rank > f.best) f.best = r.rank
    if (r.rank > d.best) d.best = r.rank
  }

  const out: HereRow[] = []
  const minOpen = here.rows.length * OPEN_SHARE
  let open = 0
  let capped = 0
  for (const d of [...dirs.values()].sort((a, b) => b.best - a.best)) {
    d.files.sort((a, b) => b.best - a.best)
    let importers = 0
    let hot = mDir === d.path
    const entries = d.files.map((f) => {
      f.rows.sort((a, b) => a.line - b.line)
      let symHit = false
      let callers = 0
      let recent = false
      let entry = false
      let isTest = false
      let fileFocus = focus.some((x) => f.path.startsWith(x))
      const syms: HereRow[] = f.rows.map((row) => {
        const fm = focusMatches(row, focus)
        const mk = mPath === row.path && mLine === row.line
        fileFocus ||= fm
        symHit ||= mk
        if (row.callers > callers) callers = row.callers
        recent ||= row.recent
        entry ||= row.entry
        isTest ||= row.is_test
        return { kind: 'symbol', key: row.moniker, row, depth: 2, focus: fm, marker: mk }
      })
      const isEdited = editedSet.has(f.path)
      hot ||= fileFocus || isEdited
      importers += f.rows[0]!.importers
      const cap = expanded.has(`more:${f.path}`) ? Infinity : SYMBOL_CAP
      const file: HereRow = {
        kind: 'file',
        key: f.path,
        path: f.path,
        depth: 1,
        open: !collapsed.has(f.path),
        importers: f.rows[0]!.importers,
        callers,
        recent,
        entry,
        isTest,
        edited: isEdited,
        focus: fileFocus,
        marker: mPath === f.path && !symHit,
        symbols: syms.length,
        hidden: Math.max(0, syms.length - cap),
      }
      return { file, syms }
    })
    // 73 §6.2: open by default at ≥ 3 % of rows or holding a focus/edited/marked file; ≤ 12 open dirs, the rest counted.
    let isOpen: boolean
    if (expanded.has(d.path)) isOpen = true
    else if (collapsed.has(d.path) || !(d.n >= minOpen || hot)) isOpen = false
    else if (open < OPEN_DIRS || expanded.has(MORE_DIRS)) isOpen = true
    else {
      isOpen = false
      capped++
    }
    if (isOpen) open++
    out.push({ kind: 'dir', key: d.path, path: d.path, depth: 0, open: isOpen, files: d.files.length, ranked: d.n, importers, coverage: false })
    if (!isOpen) continue
    for (const { file, syms } of entries) {
      out.push(file)
      if (file.kind !== 'file' || !file.open) continue
      const shown = syms.length - file.hidden
      for (let i = 0; i < shown; i++) out.push(syms[i]!)
      // 73 §6.5: a cut always carries its count.
      if (file.hidden) out.push({ kind: 'more', key: `more:${file.path}`, depth: 2, count: file.hidden, parent: file.path })
    }
  }
  for (const c of here.dirs) {
    out.push({ kind: 'dir', key: c.path, path: c.path, depth: 0, open: false, files: c.files, ranked: c.ranked_rows, importers: c.importers, coverage: true })
  }
  if (capped) out.push({ kind: 'more', key: MORE_DIRS, depth: 1, count: capped, parent: '' })
  return out
}

function same(a: MapRow, b: MapRow): boolean {
  for (const k in a) if (a[k as keyof MapRow] !== b[k as keyof MapRow]) return false
  return true
}

export function applyHereDelta(prev: MapRow[], next: MapRow[]): { rows: MapRow[]; changed: number } {
  const old = new Map(prev.map((r) => [r.moniker, r]))
  let changed = 0
  const rows = next.map((r) => {
    const p = old.get(r.moniker)
    if (p) old.delete(r.moniker)
    if (p && same(p, r)) return p
    changed++
    return r
  })
  return { rows, changed: changed + old.size }
}
