import type { SessionSummary } from '@/rpc/bindings/SessionSummary'

/** The top-level ancestor of `id` (itself when top-level or unknown). */
export function rootOf(sessions: readonly SessionSummary[], id: string | undefined): string | undefined {
  let cur = id
  for (;;) {
    const parent = sessions.find((s) => s.id === cur)?.parent
    if (!parent) return cur
    cur = parent
  }
}

/**
 * Every subagent under its top-level ancestor (all depths), keyed by the root's id — a chat row or a desk. Feeds the
 * collapsed row's running-subagent glyph and tells the Desks group which desk owns the active subagent.
 */
export function subtrees(sessions: readonly SessionSummary[]): Map<string, SessionSummary[]> {
  const out = new Map<string, SessionSummary[]>()
  for (const s of sessions) {
    if (s.parent === null) continue
    const root = rootOf(sessions, s.id)
    if (root === undefined) continue
    const list = out.get(root)
    if (list) list.push(s)
    else out.set(root, [s])
  }
  return out
}
