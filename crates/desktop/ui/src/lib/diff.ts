export type DiffLine = { op: ' ' | '-' | '+'; text: string }

/** Line diff of `a` → `b` as unified rows: unchanged, removed, added. */
export function diffLines(a: string, b: string): DiffLine[] {
  const x = a.split('\n')
  const y = b.split('\n')
  const n = x.length
  const m = y.length
  const w = m + 1
  // ponytail: O(n·m) LCS table; inputs are edit snippets, not files. Myers if that changes.
  const lcs = new Uint32Array((n + 1) * w)
  for (let i = n - 1; i >= 0; i--)
    for (let j = m - 1; j >= 0; j--)
      lcs[i * w + j] =
        x[i] === y[j] ? (lcs[(i + 1) * w + j + 1] ?? 0) + 1 : Math.max(lcs[(i + 1) * w + j] ?? 0, lcs[i * w + j + 1] ?? 0)
  const out: DiffLine[] = []
  let i = 0
  let j = 0
  while (i < n && j < m) {
    if (x[i] === y[j]) out.push({ op: ' ', text: x[i++] ?? '' }), j++
    else if ((lcs[(i + 1) * w + j] ?? 0) >= (lcs[i * w + j + 1] ?? 0)) out.push({ op: '-', text: x[i++] ?? '' })
    else out.push({ op: '+', text: y[j++] ?? '' })
  }
  while (i < n) out.push({ op: '-', text: x[i++] ?? '' })
  while (j < m) out.push({ op: '+', text: y[j++] ?? '' })
  return out
}
