// 101 D10: a unified diff (`git diff` output) → files → hunks → lines, for `UnifiedDiff.tsx`. Pure; the renderer falls
// back to `<pre>` when this returns no files.

export type DiffOp = ' ' | '+' | '-'
export type DiffRow = { op: DiffOp; text: string; old: number | null; new: number | null }
export type Hunk = { header: string; rows: DiffRow[] }
export type DiffFile = { path: string; hunks: Hunk[]; added: number; removed: number }

const HUNK = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$/

/** Strip the `a/` / `b/` prefix git puts on `---`/`+++` paths. */
const stripPrefix = (p: string) => p.replace(/^[ab]\//, '')

/**
 * Files appear on `diff --git a/x b/x` (or on a bare `--- a/x` / `+++ b/x` pair); `/dev/null` on either side still names
 * the other. Lines before the first hunk of a file (`index`, `new file mode`, …) are skipped. Returns `[]` when no hunk
 * was found, so the caller can fall back to the raw text.
 */
export function parseUnifiedDiff(text: string): DiffFile[] {
  const files: DiffFile[] = []
  let file: DiffFile | null = null
  let hunk: Hunk | null = null
  let oldN = 0
  let newN = 0
  const open = (path: string): DiffFile => {
    const f: DiffFile = { path, hunks: [], added: 0, removed: 0 }
    files.push(f)
    return f
  }
  const lines = text.split('\n')
  if (lines.at(-1) === '') lines.pop() // the newline that ends the last line, not an empty context row
  for (const line of lines) {
    const git = /^diff --git a\/(.+?) b\/(.+)$/.exec(line)
    if (git) {
      file = open(git[2] as string)
      hunk = null
      continue
    }
    if (line.startsWith('--- ')) {
      // A `---` without a preceding `diff --git` opens a file too (hand-made or `diff -u` output).
      const p = line.slice(4).trim()
      if (!file || file.hunks.length > 0) {
        file = open(p === '/dev/null' ? '' : stripPrefix(p))
        hunk = null
      }
      continue
    }
    if (line.startsWith('+++ ')) {
      // The new-side name wins for a file still without hunks (a rename shows where it went; `/dev/null` keeps the old name).
      const p = line.slice(4).trim()
      if (file && file.hunks.length === 0 && p !== '/dev/null') file.path = stripPrefix(p)
      continue
    }
    const h = HUNK.exec(line)
    if (h) {
      file ??= open('')
      oldN = Number(h[1])
      newN = Number(h[2])
      hunk = { header: line, rows: [] }
      file.hunks.push(hunk)
      continue
    }
    if (!hunk || !file) continue
    if (line.startsWith('+')) {
      hunk.rows.push({ op: '+', text: line.slice(1), old: null, new: newN++ })
      file.added++
    } else if (line.startsWith('-')) {
      hunk.rows.push({ op: '-', text: line.slice(1), old: oldN++, new: null })
      file.removed++
    } else if (line.startsWith(' ') || line === '') {
      hunk.rows.push({ op: ' ', text: line.slice(1), old: oldN++, new: newN++ })
    } else if (line.startsWith('\\')) {
      // `\ No newline at end of file` — informational, no line numbers.
      hunk.rows.push({ op: ' ', text: line, old: null, new: null })
    }
    // Anything else (`index …`, `similarity …`) ends the hunk's line run.
    else hunk = null
  }
  return files.filter((f) => f.hunks.length > 0)
}
