import { BlockNoteEditor } from '@blocknote/core'
import { schema } from '@theuth/documents/schema'
import { createElement as h } from 'react'
import { expect, test } from 'vitest'
import type { EngineProvider } from '@/lib/doc/provider'
import type { CellRef } from '@/rpc/bindings/CellRef'
import type { CellSpec } from '@/rpc/bindings/CellSpec'
import type { CellState } from '@/rpc/bindings/CellState'
import { cellOf, digest } from './grammar'
import { NoteCell, pressNote } from './NoteCell'
import { NoteHost } from './note-context'
import { noteRef } from './note-edit'
import { cells, refKey, scopeKey, scopeOf } from './store'
import { fakeApi, render, run, state } from './test-fixtures'
import { viewOf } from './view'

const DOC = 'aaaaaaaaaaaaaaaaaaaaaaaaaa'
const TEXT = '#| name: rotate-key\naws iam create-access-key --user-name frp-bot\naws iam list-access-keys --user-name frp-bot'
const spec = cellOf('bash', TEXT) as CellSpec
const ref = noteRef(DOC, spec, 'blk1')
const provider = { id: DOC, meta: { members: [{ pub: 'AbCdEf12rest', name: 'Alice' }] } } as unknown as EngineProvider

/** NoteCell's static markup with `record` as the engine's state for its ref. */
function renderNote(record: CellState | null) {
  cells.setState({ scopes: { [scopeKey(scopeOf(ref))]: { states: record ? { [refKey(ref)]: record } : {}, kernels: [], error: null, notices: {} } } })
  const editor = BlockNoteEditor.create({ schema })
  return render(
    h(
      NoteHost,
      { value: { provider, guest: null } },
      h(NoteCell, { doc: DOC, key_: ref.key, cell: spec, code: TEXT, editor: h('pre', null, TEXT), editable: true, block: 'blk1', blocknote: editor }),
    ),
  )
}

test('a press sends the Note ref, the digest of the text on screen, no edit, and member_ack only when given', async () => {
  const { api, calls } = fakeApi()
  await pressNote(api, ref, TEXT, { type: 'line', line: 2 }, 'AbCdEf12')
  await pressNote(api, ref, TEXT, { type: 'cell' }, null)
  const [first, second] = calls.map(([, args]) => args)
  expect(calls.map(([m]) => m)).toEqual(['cell_run', 'cell_run'])
  expect(first?.[0]).toEqual({ type: 'note', doc: DOC, key: { kind: 'name', value: 'rotate-key' } })
  expect(first?.[1]).toBe(await digest(TEXT))
  expect(first?.[2]).toEqual({ type: 'line', line: 2 })
  expect(first?.[3]).toMatch(/^r_[0-9A-HJKMNP-TV-Z]{26}$/)
  expect(first?.[4]).toBeNull()
  expect(first?.[5]).toBe('AbCdEf12')
  expect(second?.[5]).toBeNull()
})

test('member edit since the last run: the member by name, Review diff, the button reads "Run Alice\'s edit"', () => {
  const html = renderNote(state(ref as CellRef, { member_edit: 'AbCdEf12', latest: run({ cell: ref }) }))
  expect(html).toContain('Changed by Alice since your last run')
  expect(html).toContain('Review diff')
  expect(html).toMatch(/<button[^>]*>(?:<svg[\s\S]*?<\/svg>)?Run Alice&#x27;s edit<\/button>/)
  // The raw pub8 line of the shared chrome is replaced by the note's own row.
  expect(html).not.toContain('Changed by AbCdEf12')
})

test('member text never run: "Written by Alice · Review", still acknowledged on the press', () => {
  const html = renderNote(state(ref as CellRef, { member_edit: 'AbCdEf12' }))
  expect(html).toContain('Written by Alice')
  expect(html).toMatch(/>Review<\/button>/)
  expect(html).toContain('Run Alice&#x27;s edit')
})

test('stale: "ran an earlier version" with the code that ran one click away, and the "ran 14:02" footer', () => {
  const latest = run({ cell: ref, ended_ms: new Date(2026, 10, 3, 14, 2).getTime() })
  const s = state(ref as CellRef, { stale: true, latest })
  expect(viewOf(s, spec, false)).toBe('stale')
  const html = renderNote(s)
  expect(html).toContain('ran an earlier version')
  expect(html).toContain('show the code that ran')
  expect(html).toContain('ran 14:02')
  // Plain owner cell: the label is the button, no member row.
  expect(html).toContain('>Run</button>')
  expect(html).not.toContain('since your last run')
})

test('a note cell never folds (BlockNote needs its content mounted) and marks its chrome non-editable', () => {
  const html = renderNote(state(ref as CellRef, { latest: run({ cell: ref }) }))
  expect(viewOf(state(ref as CellRef, { latest: run({ cell: ref }) }), spec, false)).toBe('done')
  expect(html).toContain(`<pre>${TEXT}</pre>`)
  expect(html).toMatch(/contenteditable="false"/i)
  expect(html).toContain('data-note-cell=""')
})
