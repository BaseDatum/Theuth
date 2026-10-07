import { expect, test } from 'vitest'
import type { AdvisoryNote } from '@/rpc/bindings/AdvisoryNote'
import type { Entry } from '@/store/sessions'
import { collapseNotes } from './AdvisorNoteCard'
import { foldRuns } from './ToolRunCard'

const notes: AdvisoryNote[] = ['a', 'b', 'c', 'd', 'e'].map((note, i) => ({ severity: i === 3 ? 'blocker' : 'nit', note }))

test('three notes collapsed with the rest counted; expanded shows every note', () => {
  expect(collapseNotes(notes, false)).toEqual({ shown: notes.slice(0, 3), hidden: 2 })
  expect(collapseNotes(notes, true)).toEqual({ shown: notes, hidden: 0 })
  expect(collapseNotes(notes.slice(0, 2), false)).toEqual({ shown: notes.slice(0, 2), hidden: 0 })
})

test('an advisory card splits a tool run and never folds into one', () => {
  const tool = (callId: string): Entry => ({ id: callId, kind: 'tool', callId, name: 'read_file', args: {}, pending: false, child: false })
  const card: Entry = { id: 'adv', kind: 'advisory', notes: notes.slice(0, 1), seq: 3 }
  const folded = foldRuns([tool('a'), tool('b'), card, tool('c'), tool('d')])
  expect(folded.map((x) => (Array.isArray(x) ? x.map((e) => e.id) : x.id))).toEqual([['a', 'b'], 'adv', ['c', 'd']])
})
