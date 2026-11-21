import { expect, test } from 'vitest'
import type { DocSummary } from "@/rpc/bindings/method.agent.output"
import { groupNotes, newNoteProject, type NoteRow } from './groups'

const note = (id: string, project: string | null = null, kind: 'note' | 'board' = 'note'): DocSummary => ({
  id,
  kind,
  title: id,
  binding: 'note',
  created_ms: 1,
  updated_ms: 1,
  bytes: 0,
  project,
})
const keys = (rows: NoteRow[]) => rows.map((r) => r.key)

test('no project anywhere: a flat list, no headers', () => {
  expect(keys(groupNotes([note('a'), note('b', null, 'board')]))).toEqual(['a', 'b'])
})

test('notes and boards bucket under project headers in first-seen order, Ungrouped last, each note once, order kept', () => {
  const notes = [note('a'), note('b', 'beta'), note('c', 'alpha', 'board'), note('d', 'beta'), note('e')]
  const rows = groupNotes(notes)
  expect(keys(rows)).toEqual(['p:beta', 'b', 'd', 'p:alpha', 'c', 'p:', 'a', 'e'])
  expect(rows.flatMap((r) => ('project' in r ? [r.project] : []))).toEqual(['beta', 'alpha', null])
  // Collapsed groups keep their header and hide their notes; `''` is Ungrouped.
  expect(keys(groupNotes(notes, new Set(['beta', ''])))).toEqual(['p:beta', 'p:alpha', 'c', 'p:'])
  expect(groupNotes(notes, new Set(['alpha'])).find((r) => r.key === 'p:alpha')).toEqual({ key: 'p:alpha', project: 'alpha', collapsed: true })
})

test('New note goes into the one project left open when every other group is collapsed; otherwise ungrouped', () => {
  const notes = [note('a'), note('b', 'beta'), note('c', 'alpha')]
  expect(newNoteProject(groupNotes(notes))).toBeNull()
  expect(newNoteProject(groupNotes(notes, new Set(['alpha', ''])))).toBe('beta')
  // Only Ungrouped open, or everything collapsed: no project.
  expect(newNoteProject(groupNotes(notes, new Set(['alpha', 'beta'])))).toBeNull()
  expect(newNoteProject(groupNotes(notes, new Set(['alpha', 'beta', ''])))).toBeNull()
  // A flat list has no groups to be inside.
  expect(newNoteProject(groupNotes([note('a')]))).toBeNull()
})
