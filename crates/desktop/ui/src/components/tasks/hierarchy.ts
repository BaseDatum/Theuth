// The untyped task hierarchy (parent → child, any depth; the levels live in titles, e.g. "E3 …") and the per-task
// accent colour, as pure functions over the task list: the board, the detail page, the graph and its editor all read
// them from here. Every walk is cycle-safe — the engine refuses a parent loop, but a replica may still carry one.
import type { Task, TaskState } from '@/rpc/bindings/method.roster.output'

/** Accent palette: the project palette (`lib/project-style.ts` `COLORS`, engine `types::ACCENTS`) — change together. */
export const ACCENTS = ['ochre', 'carnelian', 'malachite', 'faience', 'lapis', 'nile', 'umber', 'slate', 'sand', 'plum'] as const
export type AccentKey = (typeof ACCENTS)[number]
export const isAccent = (k: string | null | undefined): k is AccentKey => !!k && (ACCENTS as readonly string[]).includes(k)
/** The theme variable of an accent: the `--project-*` palette switches with light/dark. */
export const accentColor = (k: AccentKey): string => `var(--project-${k})`

const closed = (s: TaskState): boolean => s === 'done' || s === 'cancelled'

/** A parent's children: `total` leaves cancelled ones out; `open` are neither done nor cancelled. */
export type ChildStats = { done: number; total: number; open: number }

/** Children per parent id, over every task given (children of tasks outside a filter still count). */
export function childStats(tasks: readonly Pick<Task, 'parent' | 'state'>[]): Map<string, ChildStats> {
  const out = new Map<string, ChildStats>()
  for (const t of tasks) {
    if (!t.parent || t.state === 'cancelled') continue
    const s = out.get(t.parent) ?? { done: 0, total: 0, open: 0 }
    s.total++
    if (t.state === 'done') s.done++
    else if (!closed(t.state)) s.open++
    out.set(t.parent, s)
  }
  return out
}

/** "waiting on 2 of 3 children" — the derived wait of a parent with open children (never a stored blocker). */
export const waitingOnChildren = (s: ChildStats | undefined): string | null => (s && s.open > 0 ? `waiting on ${s.open} of ${s.total} ${s.total === 1 ? 'child' : 'children'}` : null)

/** Every task under `id` (children, their children, …), not `id` itself. */
export function descendants(tasks: readonly Pick<Task, 'id' | 'parent'>[], id: string): Set<string> {
  const kids = new Map<string, string[]>()
  for (const t of tasks) if (t.parent) kids.set(t.parent, [...(kids.get(t.parent) ?? []), t.id])
  const out = new Set<string>()
  const stack = [id]
  while (stack.length) {
    for (const k of kids.get(stack.pop()!) ?? []) {
      if (k === id || out.has(k)) continue
      out.add(k)
      stack.push(k)
    }
  }
  return out
}

/** `id`'s ancestors, root first (the breadcrumb). Stops at a parent not in `tasks` or at a loop. */
export function ancestors<T extends Pick<Task, 'id' | 'parent'>>(tasks: readonly T[], id: string): T[] {
  const byId = new Map(tasks.map((t) => [t.id, t]))
  const out: T[] = []
  const seen = new Set([id])
  let cur = byId.get(id)?.parent
  while (cur && !seen.has(cur)) {
    const p = byId.get(cur)
    if (!p) break
    seen.add(cur)
    out.unshift(p)
    cur = p.parent
  }
  return out
}

/** Tasks `id` may not take as parent: itself and everything under it (that would make it its own ancestor). */
export const parentExcluded = (tasks: readonly Pick<Task, 'id' | 'parent'>[], id: string): string[] => [id, ...descendants(tasks, id)]

export type ShownAccent = { key: AccentKey; inherited: boolean }

/** Each task's accent: its own, or (with `inherit`) its nearest ancestor's, marked `inherited` (drawn faded). */
export function resolveAccents(tasks: readonly Pick<Task, 'id' | 'parent' | 'accent'>[], inherit: boolean): Map<string, ShownAccent> {
  const byId = new Map(tasks.map((t) => [t.id, t]))
  const out = new Map<string, ShownAccent>()
  for (const t of tasks) {
    if (isAccent(t.accent)) {
      out.set(t.id, { key: t.accent, inherited: false })
      continue
    }
    if (!inherit) continue
    const seen = new Set([t.id])
    let cur = t.parent ? byId.get(t.parent) : undefined
    while (cur && !seen.has(cur.id)) {
      if (isAccent(cur.accent)) {
        out.set(t.id, { key: cur.accent, inherited: true })
        break
      }
      seen.add(cur.id)
      cur = cur.parent ? byId.get(cur.parent) : undefined
    }
  }
  return out
}
