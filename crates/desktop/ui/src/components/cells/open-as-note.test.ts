import { QueryClient } from '@tanstack/react-query'
import { expect, test } from 'vitest'
import { cellOrdinals } from './CellFence'
import { historyAfter, noteMarkdown, openAsNote, runLine } from './open-as-note'
import { REPLY, SESSION, chatRef, fakeApi, run, state } from './test-fixtures'

/** A fake engine remembering one note per reply, with `docs` as the live (untrashed) notes. */
function engine(docs: string[]) {
  const remembered = new Map<string, string>()
  let made = 0
  return fakeApi({
    cell_reply_note_get: (s, e) => remembered.get(`${s}/${e}`) ?? null,
    cell_reply_note_set: (s, e, d) => void remembered.set(`${s}/${e}`, d as string),
    doc_list: () => docs.map((id) => ({ id })),
    doc_create: () => {
      const id = `doc${++made}`
      docs.push(id)
      return { id }
    },
  })
}

test('Open as note twice — even a double click — makes one note and opens it both times (171F B9)', async () => {
  const docs: string[] = []
  const { api, calls } = engine(docs)
  const opened: string[] = []
  const press = () => openAsNote(api, new QueryClient(), SESSION, cellOrdinals(REPLY, '42', false), [], (id) => opened.push(id))
  await Promise.all([press(), press()])
  await press()
  expect(calls.filter(([m]) => m === 'doc_create')).toHaveLength(1)
  expect(opened).toEqual(['doc1', 'doc1'])
  expect(calls.find(([m]) => m === 'cell_reply_note_set')?.[1]).toEqual([SESSION, '42', 'doc1'])

  // The note was trashed: the next press makes a new one and remembers it instead.
  docs.length = 0
  await press()
  expect(calls.filter(([m]) => m === 'doc_create')).toHaveLength(2)
  expect(opened.at(-1)).toBe('doc2')
  expect(calls.filter(([m]) => m === 'cell_reply_note_set').at(-1)?.[1]).toEqual([SESSION, '42', 'doc2'])
})

test('171F4 T2-3: Open as note writes one output per run, no fence for a quiet one, and the engine\'s run line', async () => {
  const end = new Date(2026, 10, 4, 14, 36, 59).getTime()
  const loud = run({ id: 'r_loud', cell: chatRef(1), ended_ms: end })
  const quiet = run({ id: 'r_quiet', cell: chatRef(2), ended_ms: end, secs: 0.05, expect: [], output_bytes: 0 })
  const { api } = fakeApi({ cell_output: (id) => ({ text: id === 'r_loud' ? 'Exported to /tmp/x.json\n' : '', total: id === 'r_loud' ? 24 : 0 }) })
  const md = await noteMarkdown(api, cellOrdinals(REPLY, '42', false), [state(chatRef(1), { latest: loud }), state(chatRef(2), { latest: quiet }), null])
  expect(md.match(/```output/g)).toHaveLength(1)
  expect(md).toContain('```output\nExported to /tmp/x.json\n```\nran 14:36 · exit 0 · 41 s · expect ✓\n')
  // The end time to the minute, seconds as the engine writes them (`doc_read` of a note run reads the same).
  expect(md).toContain('pulumi preview -s ir-prod1 --refresh=false --diff\n```\n\nran 14:36 · exit 0 · 0.1 s\n')
  expect(runLine(run({ state: 'timeout', exit: { type: 'code', value: 130 }, secs: 5.26, ended_ms: end, expect: [{ item: { kind: 'text', value: 'x' }, met: false }] }))).toBe(
    'ran 14:36 · exit timeout · 5.3 s · expect ✗',
  )
})

test('171F4 T2-3: run in the note, a cell\'s history (output block and run line, or the line alone) gives way', () => {
  const p = (id: string, text: string) => ({ id, type: 'paragraph', content: [{ type: 'text', text }] })
  const code = (id: string, language: string) => ({ id, type: 'codeBlock', props: { language }, content: [] })
  // The note Open as note made from the reply above: export (printed), preview (quiet), apply (never ran in chat).
  const blocks = [
    p('intro', 'Three steps.'),
    code('c1', 'bash'),
    code('o1', 'output'),
    p('r1', 'ran 14:36 · exit 0 · 41 s · expect ✓'),
    code('c2', 'bash'),
    p('r2', 'ran 14:36 · exit 0 · 0.1 s'),
    code('c3', 'bash'),
    p('after', 'ran out of ideas · or not'),
  ]
  expect(historyAfter(blocks, 'c1')).toEqual(['o1', 'r1'])
  expect(historyAfter(blocks, 'c2')).toEqual(['r2'])
  expect(historyAfter(blocks, 'c3')).toEqual([])
  // Nested under a heading's children, as BlockNote nests.
  expect(historyAfter([{ id: 'h', type: 'heading', children: blocks }], 'c1')).toEqual(['o1', 'r1'])
})
