import { diffLines, type DiffLine } from './diff'

export type DiffRow = { kind: 'line' | 'hunk' | 'note'; text: string; op?: DiffLine['op']; old?: number; next?: number }
export type DiffFile = { path: string; rows: DiffRow[] }
export type DiffModel = { files: DiffFile[]; path: string; added: number; removed: number }
export type DiffPair = { path: string; old: string; new: string; start?: number }

const linesOf = (s: string): string[] => s === '' ? [] : s.replace(/\r\n/g, '\n').replace(/\n$/, '').split('\n')
const object = (v: unknown): Record<string, unknown> => v !== null && typeof v === 'object' && !Array.isArray(v) ? v as Record<string, unknown> : {}
const pathOf = (s: string) => s.split('\t')[0]!.replace(/^[ab]\//, '')

function modelOf(files: DiffFile[]): DiffModel {
  let added = 0, removed = 0
  for (const file of files) for (const row of file.rows) {
    if (row.op === '+') added++
    if (row.op === '-') removed++
  }
  return { files, path: [...new Set(files.map((f) => f.path))].join(' → '), added, removed }
}

/** Hunk lengths distinguish file headers from removed/added text beginning with --/++. */
export function parseUnified(text: string, fallback = ''): DiffModel {
  const files: DiffFile[] = []
  const lines = text.replace(/\r\n/g, '\n').split('\n')
  let file: DiffFile | undefined
  let old = 0, next = 0, leftOld = 0, leftNew = 0
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]!
    if (line.startsWith('diff --git ')) { file = undefined; leftOld = leftNew = 0; continue }
    if (!leftOld && !leftNew && line.startsWith('--- ') && lines[i + 1]?.startsWith('+++ ')) {
      const before = pathOf(line.slice(4)), after = pathOf(lines[++i]!.slice(4))
      file = { path: after === '/dev/null' ? before : after, rows: [] }
      files.push(file)
      continue
    }
    const hunk = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/.exec(line)
    if (hunk) {
      if (!file) { file = { path: fallback, rows: [] }; files.push(file) }
      old = Number(hunk[1]); next = Number(hunk[3])
      leftOld = Number(hunk[2] ?? 1); leftNew = Number(hunk[4] ?? 1)
      file.rows.push({ kind: 'hunk', text: line })
    } else if (file && (line.startsWith('\\ No newline at end of file') || /^\[diff truncated:/.test(line))) {
      file.rows.push({ kind: 'note', text: line })
    } else if (file && (leftOld > 0 || leftNew > 0)) {
      const op = line[0]
      if ((op === '-' && leftOld > 0) || (op === '+' && leftNew > 0) || (op === ' ' && leftOld > 0 && leftNew > 0)) {
        file.rows.push({ kind: 'line', text: line.slice(1), op, old: op === '+' ? undefined : old++, next: op === '-' ? undefined : next++ })
        if (op !== '+') leftOld--
        if (op !== '-') leftNew--
      }
    }
  }
  return modelOf(files)
}

/** Keep the existing snippet LCS bounded when arguments carry entire files. */
function rowsOf(before: string[], after: string[]): DiffLine[] {
  if (!before.length) return after.map((text) => ({ op: '+', text }))
  if (!after.length) return before.map((text) => ({ op: '-', text }))
  if (before.length * after.length <= 4_000_000) return diffLines(before.join('\n'), after.join('\n'))
  // ponytail: large argument-only diffs replace the changed middle; Myers if finer large-file alignment matters.
  let head = 0, tail = 0
  while (head < before.length && head < after.length && before[head] === after[head]) head++
  while (tail < before.length - head && tail < after.length - head && before[before.length - tail - 1] === after[after.length - tail - 1]) tail++
  return [
    ...before.slice(0, head).map((text): DiffLine => ({ op: ' ', text })),
    ...before.slice(head, before.length - tail).map((text): DiffLine => ({ op: '-', text })),
    ...after.slice(head, after.length - tail).map((text): DiffLine => ({ op: '+', text })),
    ...before.slice(before.length - tail).map((text): DiffLine => ({ op: ' ', text })),
  ]
}

export function pairsDiff(pairs: DiffPair[]): DiffModel {
  return modelOf(pairs.map((pair) => {
    const before = linesOf(pair.old), after = linesOf(pair.new)
    let old = pair.start ?? 1, next = old
    const oldEnd = old + before.length - 1, newEnd = next + after.length - 1
    const body: DiffRow[] = rowsOf(before, after).flatMap((row): DiffRow[] => {
      const line: DiffRow = { kind: 'line', ...row, old: row.op === '+' ? undefined : old++, next: row.op === '-' ? undefined : next++ }
      // Equal final text with a changed final newline is still a line replacement.
      if (row.op === ' ' && line.old === oldEnd && line.next === newEnd && pair.old.endsWith('\n') !== pair.new.endsWith('\n')) {
        return [{ ...line, op: '-', next: undefined }, { ...line, op: '+', old: undefined }]
      }
      return [line]
    })
    const rows: DiffRow[] = body.some((r) => r.op !== ' ') ? [
      { kind: 'hunk', text: `@@ -${before.length ? pair.start ?? 1 : (pair.start ?? 1) - 1},${before.length} +${after.length ? pair.start ?? 1 : (pair.start ?? 1) - 1},${after.length} @@` },
      ...body.flatMap((row): DiffRow[] => [row, ...((row.old === oldEnd && !pair.old.endsWith('\n')) || (row.next === newEnd && !pair.new.endsWith('\n')) ? [{ kind: 'note' as const, text: '\\ No newline at end of file' }] : [])]),
    ] : []
    return { path: pair.path, rows }
  }))
}

const NATIVE = '\n<theuth-diff-result>\n'
const ACP = '\n<theuth-acp-diff>\n'
function resultData(output: string): Record<string, unknown> {
  const start = output.lastIndexOf(NATIVE)
  const text = start < 0 ? output : output.slice(start + NATIVE.length).split('\n</theuth-diff-result>')[0]!
  try {
    const value = object(JSON.parse(text))
    return value.structured ? object(value.structured) : value
  } catch { return {} }
}

function argumentPairs(name: string, args: Record<string, unknown>, data: Record<string, unknown>): DiffPair[] {
  const path = typeof args.path === 'string' ? args.path : typeof data.path === 'string' ? data.path : ''
  if (name === 'write_file' && typeof args.content === 'string') return [{ path, old: '', new: args.content }]
  const edits = Array.isArray(args.edits) ? args.edits.map(object) : [args]
  return edits.flatMap((edit): DiffPair[] => {
    if (typeof edit.new !== 'string') return []
    if (typeof edit.old === 'string') return [{ path, old: edit.old, new: edit.new }]
    if (typeof edit.range !== 'string') return []
    const range = /^L(\d+)(?:-L?(\d+))?@/.exec(edit.range)
    // Native snapshot ids are hashes, not text. Only use an explicit text-bearing snapshot.
    const snapshot = object(data.snapshot)
    const text = typeof snapshot.text === 'string' ? snapshot.text : typeof data.snapshot_text === 'string' ? data.snapshot_text : undefined
    const start = range ? Number(range[1]) : 1
    const old = text !== undefined && range ? linesOf(text).slice(start - 1, Number(range[2] ?? range[1])).join('\n') : ''
    return [{ path, old, new: edit.new, start }]
  })
}

/** Native result diff, then arguments, then the explicitly marked ACP diff. */
export function toolDiff(name: string, rawArgs: unknown, result?: { output: string; isError?: boolean }): DiffModel | null {
  const args = object(rawArgs), output = result?.output ?? '', data = resultData(output)
  const acpAt = output.indexOf(ACP)
  const family = ['edit', 'write_file', 'move_text', 'copy_text', 'indent'].includes(name.toLowerCase()) || args.kind === 'edit'
  if (!family && acpAt < 0) return null
  const path = typeof args.path === 'string' ? args.path : typeof data.path === 'string' ? data.path : ''
  const own = [data, ...(Array.isArray(data.files) ? data.files.map(object) : [])].filter((v) => typeof v.diff === 'string')
  const files = own.flatMap((v) => parseUnified(v.diff as string, typeof v.path === 'string' ? v.path : path).files)
  let model: DiffModel | undefined
  if (files.length) model = modelOf(files)
  // New native files intentionally return an empty diff and zero counters; their arguments are the additions.
  else if (own.length && data.created !== true) model = modelOf(own.map((v) => ({ path: typeof v.path === 'string' ? v.path : path, rows: [] })))
  else if (acpAt < 0) {
    const parsed = parseUnified(output, path)
    if (parsed.files.length) model = parsed
  }
  if (!model) {
    const pairs = argumentPairs(name.toLowerCase(), args, data)
    if (pairs.length) model = pairsDiff(pairs)
  }
  if (!model && acpAt >= 0) model = modelOf([...output.matchAll(/\n<theuth-acp-diff>\n([\s\S]*?)\n<\/theuth-acp-diff>/g)].flatMap((match) => parseUnified(match[1]!, path).files))
  if (!model?.files.length) return null
  if (data.created !== true) {
    if (typeof data.added === 'number') model.added = data.added
    if (typeof data.removed === 'number') model.removed = data.removed
    // Pre-carrier native edit/write receipts already report complete counts even when the diff is truncated.
    const counts = /\(\+(\d+) −(\d+) lines\)/.exec(output)
    if (!own.length && counts) { model.added = Number(counts[1]); model.removed = Number(counts[2]) }
  }
  return model
}

export type WordPart = { text: string; changed: boolean }
export function wordChanges(before: string, after: string): [WordPart[], WordPart[]] {
  const tokenize = (s: string) => s.match(/\s+|[\p{L}\p{N}_]+|[^\s\p{L}\p{N}_]/gu) ?? []
  const a = tokenize(before), b = tokenize(after)
  // ponytail: skip word alignment on minified/very long lines; line colours still show the whole change.
  if (before.length + after.length > 8_000 || a.length * b.length > 100_000) return [[{ text: before, changed: false }], [{ text: after, changed: false }]]
  const rows = rowsOf(a, b)
  return [rows.filter((r) => r.op !== '+').map((r) => ({ text: r.text, changed: r.op === '-' })), rows.filter((r) => r.op !== '-').map((r) => ({ text: r.text, changed: r.op === '+' }))]
}

/** One row of a side-by-side diff, as indices into the file's `DiffRow`s: a hunk header or note across both sides,
 * or a line pair — `left` the old side (context or removal), `right` the new (context or addition); `null` is the
 * filler cell where the other side has no counterpart. A context row is the same index on both sides. */
export type SplitRow = { kind: 'hunk' | 'note'; row: number } | { kind: 'line'; left: number | null; right: number | null }

/**
 * Align `rows` side by side: context lines pair with themselves; a run of removals pairs in order with the run of
 * additions right after it (never across context or a hunk header), the longer run's rest against filler. Notes
 * inside a run (`\ No newline at end of file`) follow the run's pairs.
 */
export function splitRows(rows: readonly DiffRow[]): SplitRow[] {
  const out: SplitRow[] = []
  for (let i = 0; i < rows.length; i++) {
    const row = rows[i]!
    if (row.kind !== 'line') { out.push({ kind: row.kind, row: i }); continue }
    if (row.op !== '-' && row.op !== '+') { out.push({ kind: 'line', left: i, right: i }); continue }
    const removed: number[] = [], added: number[] = [], notes: number[] = []
    const take = (op: '-' | '+', into: number[]) => {
      for (; i < rows.length && (rows[i]!.op === op || rows[i]!.kind === 'note'); i++) (rows[i]!.kind === 'note' ? notes : into).push(i)
    }
    take('-', removed)
    take('+', added)
    i--
    for (let n = 0; n < Math.max(removed.length, added.length); n++) out.push({ kind: 'line', left: removed[n] ?? null, right: added[n] ?? null })
    for (const note of notes) out.push({ kind: 'note', row: note })
  }
  return out
}

/** The changed words of each paired removal/addition (`splitRows`' pairs), by row index; unpaired rows have none. */
export function marksOf(rows: readonly DiffRow[]): Map<number, WordPart[]> {
  const marks = new Map<number, WordPart[]>()
  for (const s of splitRows(rows)) {
    if (s.kind !== 'line' || s.left === null || s.right === null || s.left === s.right) continue
    const [before, after] = wordChanges(rows[s.left]!.text, rows[s.right]!.text)
    marks.set(s.left, before)
    marks.set(s.right, after)
  }
  return marks
}
