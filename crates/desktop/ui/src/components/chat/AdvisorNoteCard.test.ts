import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import type { AdvisoryNote } from "@/rpc/bindings/method.agent.output"
import type { Entry } from '@/store/sessions'
import { AdvisorNoteCard, collapseNotes } from './AdvisorNoteCard'
import { DisclosureScope } from './disclosure'
import { useToolRuns } from '@/lib/tool-runs'
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

test('R1: a consultation card titles by its question, wears a Consulted badge and bodies the answer without a severity chip', () => {
  // The body mounts only while the card is open (171F B13: a store-backed disclosure); open it in the store.
  useToolRuns.setState({ disclosed: { 'a|advisory': true } })
  const card = (props: Parameters<typeof AdvisorNoteCard>[0]) => renderToStaticMarkup(createElement(DisclosureScope, { value: 'a' }, createElement(AdvisorNoteCard, props)))
  const html = card({ notes: [{ severity: 'advice', note: 'No: the column is gone.' }], question: 'Retry the migration?', at: 1_700_000_000_000 })
  expect(html).toContain('>Consulted</span>')
  expect(html).toContain('>Retry the migration?</span>')
  expect(html).not.toContain('Advisor · 1 note')
  expect(html).not.toContain('>advice</span>')
  expect(html).toMatch(/<li[^>]*><span[^>]*>No: the column is gone\.<\/span><\/li>/)
  const review = card({ notes: [{ severity: 'nit', note: 'prefer let' }] })
  expect(review).toContain('Advisor · 1 note')
  expect(review).not.toContain('Consulted')
  expect(review).toContain('>nit</span>')
})
