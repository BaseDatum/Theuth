import { QueryClient } from '@tanstack/react-query'
import { expect, test } from 'vitest'
import { cellOrdinals } from './CellFence'
import { openAsNote } from './open-as-note'
import { REPLY, SESSION, fakeApi } from './test-fixtures'

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
