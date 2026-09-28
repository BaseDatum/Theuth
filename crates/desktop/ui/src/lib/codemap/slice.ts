/** 73 §4 — pure reducer for `SessionSlice.codemap`; called from `store/reduce.ts`. */
import type { CodeToolRan, CodemapSlice } from './types'

export const MAX_ROOTS = 8
export const MAX_TRAIL = 20
export const MAX_EDITED = 200

export const emptyCodemap = (): CodemapSlice => ({ roots: [], last: null, trail: [], edited: {} })

/** The one thing a call was about: definition anchor, outlined path, first focus entry, or the impact range. */
export function trailTarget(e: CodeToolRan): string | null {
  const s = e.summary
  if (s.definition) return `${s.definition.path}#L${s.definition.line_start}`
  if (s.path) return s.path
  if (s.focus?.[0]) return s.focus[0]
  const range = s.impact?.range
  if (range) return `${range.old}..${range.new}`
  const files = e.args.files
  if (Array.isArray(files) && typeof files[0] === 'string') return files[0]
  return null
}

const bounded = <T,>(xs: T[], max: number) => (xs.length > max ? xs.slice(xs.length - max) : xs)

export function codeToolRan(c: CodemapSlice, e: CodeToolRan, at = Date.now()): CodemapSlice {
  const known = c.roots.filter((r) => r !== '?')
  const roots = known.includes(e.root) ? known : bounded([...known, e.root], MAX_ROOTS)
  return {
    ...c,
    roots,
    last: { root: e.root, tool: e.tool, callId: e.call_id, args: e.args, summary: e.summary, seq: e.seq },
    trail: [{ root: e.root, tool: e.tool, callId: e.call_id, target: trailTarget(e), seq: e.seq, at, args: e.args, summary: e.summary }, ...c.trail.filter((t) => t.callId !== e.call_id)].slice(0, MAX_TRAIL),
  }
}

/** `edit` / `write_file` started: record the root-relative path under the root it belongs to (absolute paths pick their root; relative ones the last root). */
export function codeEdited(c: CodemapSlice, name: string, args: unknown): CodemapSlice {
  if (name !== 'edit' && name !== 'write_file') return c
  const path = (args as { path?: unknown } | null)?.path
  if (typeof path !== 'string' || !path) return c
  let root = c.last?.root ?? c.roots.find((r) => r !== '?')
  let rel = path
  for (const r of c.roots) {
    if (r !== '?' && path.startsWith(`${r}/`)) {
      root = r
      rel = path.slice(r.length + 1)
      break
    }
  }
  if (!root || rel.startsWith('/')) return c
  const cur = c.edited[root] ?? []
  if (cur.includes(rel)) return c
  return { ...c, edited: { ...c.edited, [root]: bounded([...cur, rel], MAX_EDITED) } }
}

/** History load: a `code_*` tool ran before we listened — the root is unknown until the next `CodeToolRan`. */
export function codeFromHistory(c: CodemapSlice, names: Iterable<string>): CodemapSlice {
  if (c.roots.length) return c
  for (const n of names) if (n.startsWith('code_')) return { ...c, roots: ['?'] }
  return c
}
