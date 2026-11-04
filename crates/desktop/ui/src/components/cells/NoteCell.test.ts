import { BlockNoteEditor } from '@blocknote/core'
import { schema } from '@theuth/documents/schema'
import { createElement as h } from 'react'
import { expect, test, vi } from 'vitest'
import type { EngineProvider } from '@/lib/doc/provider'
import { disclosureKey, useToolRuns } from '@/lib/tool-runs'
import type { CellRef } from '@/rpc/bindings/CellRef'
import type { CellSpec } from '@/rpc/bindings/CellSpec'
import type { CellState } from '@/rpc/bindings/CellState'
import type { PassEnd } from '@/rpc/bindings/PassEnd'
import { cellOf, digest } from './grammar'
import { NoteCell, pressNote, rereadAfterEdit } from './NoteCell'
import { NoteHost } from './note-context'
import { noteRef, withPython } from './note-edit'
import { cells, refKey, register, scopeKey, scopeOf } from './store'
import { fakeApi, render, run, state } from './test-fixtures'
import { viewOf } from './view'

const DOC = 'aaaaaaaaaaaaaaaaaaaaaaaaaa'
const TEXT = '#| name: rotate-key\naws iam create-access-key --user-name frp-bot\naws iam list-access-keys --user-name frp-bot'
const spec = cellOf('bash', TEXT) as CellSpec
const ref = noteRef(DOC, spec, 'blk1')
const provider = { id: DOC, meta: { members: [{ pub: 'AbCdEf12rest', name: 'Alice' }] } } as unknown as EngineProvider

/** NoteCell's static markup with `record` as the engine's state for its ref and `pass` as the scope's last pass. */
function renderNote(record: CellState | null, pass: PassEnd | null = null) {
  cells.setState({ scopes: { [scopeKey(scopeOf(ref))]: { states: record ? { [refKey(ref)]: record } : {}, kernels: [], error: null, notices: {}, pass } } })
  return mountNote()
}

/** NoteCell's static markup over the cells store as it is. */
function mountNote() {
  const editor = BlockNoteEditor.create({ schema })
  return render(
    h(
      NoteHost,
      { value: { provider, guest: null } },
      h(NoteCell, { doc: DOC, key_: ref.key, cell: spec, code: TEXT, editor: h('pre', null, TEXT), editable: true, block: 'blk1', blocknote: editor }),
    ),
  )
}

test('171F4 N1: an edit re-reads the record once it settles; the footer then says "ran an earlier version", a revert clears it', async () => {
  vi.useFakeTimers()
  try {
    const latest = run({ cell: ref, ended_ms: new Date(2026, 10, 4, 15, 13).getTime() })
    let onScreen = TEXT
    // The engine: stale while the note's text is not the code the run ran.
    const { api, calls } = fakeApi({ cell_state: () => ({ cells: [state(ref as CellRef, { latest, stale: onScreen !== TEXT })], kernels: [] }) })
    const reads = () => calls.filter(([m]) => m === 'cell_state').length
    const off = register(api, scopeOf(ref), { ref, spec, label: 'rotate-key' })
    await vi.advanceTimersByTimeAsync(0)
    expect(reads()).toBe(1)
    expect(mountNote()).toContain('ran 15:13</span>')
    const reread = rereadAfterEdit(scopeOf(ref))
    // A trailing space, a new line, a comment: typed 300 ms apart, no read per keystroke.
    for (const typed of [`${TEXT} `, `${TEXT} \n`, `${TEXT} \n# why`]) {
      onScreen = typed
      reread.edit()
      await vi.advanceTimersByTimeAsync(300)
    }
    expect(reads()).toBe(1)
    await vi.advanceTimersByTimeAsync(500)
    expect(reads()).toBe(2)
    expect(mountNote()).toContain('ran 15:13 · ran an earlier version</span>')
    onScreen = TEXT
    reread.edit()
    await vi.advanceTimersByTimeAsync(800)
    expect(reads()).toBe(3)
    expect(mountNote()).not.toContain('ran an earlier version')
    off()
  } finally {
    vi.useRealTimers()
  }
})

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

test('Monty refused the cell (171F B10): its reason, and Use Python rewrites the header to `python: auto`', () => {
  const monty = { code: 'monty_insufficient', severity: 'error' as const, message: "Monty can't run this cell: line 2: open() is not available", key: 'python' }
  const html = renderNote(state(ref as CellRef, { problems: [monty] }))
  expect(html).toContain('Monty can&#x27;t run this cell: line 2: open() is not available')
  expect(html).toMatch(/>Use Python<\/button>/)
  expect(renderNote(state(ref as CellRef, { problems: [{ ...monty, code: 'empty', message: 'no code after the header' }] }))).not.toContain('Use Python')
  expect(withPython('#| name: t\n#| python: monty\nopen("x")', 'auto')).toBe('#| name: t\n#| python: auto\nopen("x")')
})

test('the cell a Run all pass stopped at says so and is ringed (171F B11), until a newer pass runs', () => {
  const pass: PassEnd = { pass: 'p_1', total: 3, ran: 1, stopped: { cell: ref, why: 'exit 1', before: false } }
  const failed = state(ref as CellRef, { latest: run({ cell: ref, pass: 'p_1', state: 'failed', exit: { type: 'code', value: 1 } }) })
  const html = renderNote(failed, pass)
  expect(html).toContain('Run all stopped here: exit 1')
  expect(html).toContain('ring-danger')
  expect(renderNote(failed, { ...pass, stopped: null })).not.toContain('Run all stopped here')
  const rerun = state(ref as CellRef, { latest: run({ cell: ref, pass: 'p_2', state: 'running', exit: null }) })
  expect(renderNote(rerun, pass)).not.toContain('Run all stopped here')
})

test('171F4 N3: in a note the "#|" pill unfolds the header in the editor — no read-only copy of it as in chat', () => {
  const pill = disclosureKey(refKey(ref), 'raw')
  useToolRuns.getState().setDisclosed(pill, true)
  try {
    const html = renderNote(state(ref as CellRef, { latest: run({ cell: ref }) }))
    expect(html).toContain('aria-pressed="true"')
    expect(html).not.toContain('#| name: rotate-key</pre>')
  } finally {
    useToolRuns.getState().setDisclosed(pill, false)
  }
})
