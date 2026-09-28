import type { HereLens, MapRow } from '@/lib/codemap/types'

const KINDS = ['function', 'method', 'struct', 'trait', 'enum', 'const', 'module']

/** Deterministic (LCG) §5-shaped HereLens: n rows over ≤ 60 files in 15 dirs; `skew` > 1 piles rows into low dirs (1 = uniform). */
export function bigHere(n: number, skew = 3): HereLens {
  let s = 42
  const rnd = () => (s = (s * 1664525 + 1013904223) >>> 0) / 2 ** 32
  const lastLine = new Map<string, number>()
  const rows: MapRow[] = []
  for (let i = 0; i < n; i++) {
    const d = Math.floor(15 * rnd() ** skew)
    const path = `crates/d${d}/src/f${Math.floor(4 * rnd())}.rs`
    const line = (lastLine.get(path) ?? 0) + 1 + Math.floor(rnd() * 40)
    lastLine.set(path, line)
    const kind = KINDS[i % KINDS.length]!
    const name = `sym${i}`
    rows.push({
      moniker: `rust:${path}#${name}(${kind})`,
      path,
      line,
      line_end: line + Math.floor(rnd() * 30),
      kind,
      name,
      signature: `pub fn ${name}()`,
      rank: 1 - i / n,
      exported: rnd() < 0.7,
      importers: Math.floor(rnd() * 40),
      callers: Math.floor(rnd() * 20),
      recent: rnd() < 0.1,
      entry: rnd() < 0.05,
      is_test: d === 14,
    })
  }
  // §5 order: files by best rank (= first appearance, ranks descend), symbols by line.
  const first = new Map<string, number>()
  rows.forEach((r, i) => first.has(r.path) || first.set(r.path, i))
  rows.sort((a, b) => first.get(a.path)! - first.get(b.path)! || a.line - b.line)
  return {
    meta: { root: '/tmp/big', seq: 1, indexed_files: 1200, stale: false, pending: 0, suspect_root: false, hint: null, truncated_walk: false, git_head: 'deadbeef', ms: 3 },
    rows,
    dropped: 0,
    dirs: [
      { path: 'crates/desktop/ui', files: 412, ranked_rows: 0, importers: 0 },
      { path: 'packages/apps', files: 61, ranked_rows: 0, importers: 1 },
      { path: 'crates/voice', files: 7, ranked_rows: 0, importers: 2 },
    ],
    personalised: false,
    cache_hit: true,
  }
}
