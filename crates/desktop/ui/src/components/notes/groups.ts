import type { DocSummary } from "@/rpc/bindings/method.agent.output"

/** A notes-list row: a collapsible project header (`project: null` = Ungrouped) or a note. */
export type NoteRow = { key: string; project: string | null; collapsed: boolean } | { key: string; note: DocSummary }

/**
 * The notes list by project (the sessions' labels, one level): `notes` in their order (newest first) bucketed by
 * `project` in first-seen order, `Ungrouped` last, each under a header. `collapsed` holds project
 * labels (`''` = Ungrouped) whose notes are hidden. With no project on any note there are no headers — a flat list,
 * as the chat list does.
 */
export function groupNotes(notes: readonly DocSummary[], collapsed: ReadonlySet<string> = new Set()): NoteRow[] {
  if (!notes.some((n) => n.project)) return notes.map((n) => ({ key: n.id, note: n }))
  const buckets = new Map<string | null, DocSummary[]>()
  for (const n of notes) {
    const p = n.project ?? null
    const list = buckets.get(p)
    if (list) list.push(n)
    else buckets.set(p, [n])
  }
  const ungrouped = buckets.get(null)
  buckets.delete(null)
  if (ungrouped) buckets.set(null, ungrouped)
  const rows: NoteRow[] = []
  for (const [project, list] of buckets) {
    const isCollapsed = collapsed.has(project ?? '')
    rows.push({ key: `p:${project ?? ''}`, project, collapsed: isCollapsed })
    if (!isCollapsed) rows.push(...list.map((n) => ({ key: n.id, note: n })))
  }
  return rows
}

/**
 * Where the list's own New note files the note: the one project left open when every other group (Ungrouped
 * included) is collapsed — the reader is working inside it. Otherwise ungrouped (`null`); a header's `+` names its
 * project directly.
 */
export function newNoteProject(rows: readonly NoteRow[]): string | null {
  const open = rows.flatMap((r) => ('project' in r && !r.collapsed ? [r.project] : []))
  const headers = rows.filter((r) => 'project' in r).length
  return headers > 1 && open.length === 1 ? (open[0] ?? null) : null
}
