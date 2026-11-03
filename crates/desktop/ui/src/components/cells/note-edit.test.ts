import { BlockNoteEditor } from '@blocknote/core'
import { schema } from '@theuth/documents/schema'
import { expect, test } from 'vitest'
import { diagramOf } from '@/components/editor/codeLanguages'
import type { CellSpec } from '@/rpc/bindings/CellSpec'
import { cellOf } from './grammar'
import { CELL_STARTER, firstDefaults, lineDiff, makeCell, makePlain, memberName, modeOf, noteRef, routeOf, slugOf, withPython, withoutHeader } from './note-edit'

/** A headless editor over the documents schema (the UI's code block has the same config) holding `blocks`. */
function editorWith(blocks: unknown[]) {
  const editor = BlockNoteEditor.create({ schema })
  editor.replaceBlocks(editor.document, blocks as never)
  let changes = 0
  editor.onChange(() => changes++)
  return { editor, changes: () => changes }
}
const textOf = (b: { content?: unknown }) => (b.content as { text: string }[]).map((c) => c.text).join('')

test('routing before the diagram branch: cell, guest cell, defaults line, plain code, diagrams untouched', () => {
  const owner = { guest: null }
  const guest = { guest: {} }
  expect(routeOf('bash', '#| name: up\nmake up', owner, false)).toMatchObject({ kind: 'cell', spec: { lang: 'bash', options: { name: 'up' } } })
  expect(routeOf('python', '#| python: .venv\nprint(1)', guest, true)).toMatchObject({ kind: 'guest', spec: { lang: 'python' } })
  // A plain snippet, an unrunnable language with a header, BlockNote's `shellscript`: the code path as today.
  expect(routeOf('bash', 'make up', owner, false)).toEqual({ kind: 'code' })
  expect(routeOf('rust', '#| name: x\nfn main() {}', owner, false)).toEqual({ kind: 'code' })
  expect(routeOf('shellscript', '#| name: x\nls', owner, false)).toEqual({ kind: 'code' })
  // Mermaid is never a cell — it keeps the diagram branch.
  expect(routeOf('mermaid', '#| name: x\nflowchart LR\n  A --> B', owner, false)).toEqual({ kind: 'code' })
  expect(diagramOf('mermaid')).toBe('mermaid')
  // `cells` (first word) is the defaults line until the caret is in it; outside a note editor nothing changes.
  expect(routeOf('cells', '#| cwd: ~/x', owner, false)).toEqual({ kind: 'defaults' })
  expect(routeOf('cells extra', '#| cwd: ~/x', owner, false)).toEqual({ kind: 'defaults' })
  expect(routeOf('cells', '#| cwd: ~/x', owner, true)).toEqual({ kind: 'code' })
  expect(routeOf('bash', '#| name: up\nmake up', null, false)).toEqual({ kind: 'code' })
})

test('Make it a cell: one transaction, name from the nearest heading, shellscript becomes bash; Make it plain code undoes it', () => {
  const { editor, changes } = editorWith([
    { type: 'heading', content: 'Old section' },
    { type: 'heading', props: { level: 2 }, content: 'Deploy to ir-prod1!' },
    { type: 'codeBlock', props: { language: 'shellscript' }, content: 'pulumi up -s ir-prod1\necho done' },
  ])
  const block = () => editor.document[2] as unknown as { id: string; props: { language: string }; content: unknown }
  makeCell(editor, block())
  expect(changes()).toBe(1)
  expect(block().props.language).toBe('bash')
  expect(textOf(block())).toBe('#| name: deploy-to-ir-prod1\npulumi up -s ir-prod1\necho done')
  expect(cellOf(block().props.language, textOf(block()))?.options.name).toBe('deploy-to-ir-prod1')

  makePlain(editor, block())
  expect(changes()).toBe(2)
  expect(textOf(block())).toBe('pulumi up -s ir-prod1\necho done')
})

test('Make it plain code removes every header line and keeps the code byte for byte; no heading → step-1', () => {
  expect(withoutHeader('#| name: a\n#| danger\n#| cwd: /tmp\n  indented\n#| not header\n', 'bash')).toBe('  indented\n#| not header\n')
  expect(withoutHeader('plain', 'bash')).toBe('plain')
  const { editor } = editorWith([{ type: 'codeBlock', props: { language: 'python' }, content: 'print(1)' }])
  makeCell(editor, editor.document[0] as never)
  expect(textOf(editor.document[0] as never)).toBe('#| name: step-1\nprint(1)')
  expect(slugOf('  Ünïcode — 2 steps  ')).toBe('unicode-2-steps')
  expect(slugOf('x'.repeat(60))).toHaveLength(48)
})

test('slash "Cell" inserts a bash cell named step-1', () => {
  expect(CELL_STARTER).toEqual({ type: 'codeBlock', props: { language: 'bash' }, content: '#| name: step-1' })
  expect(cellOf(CELL_STARTER.props.language, CELL_STARTER.content)).toMatchObject({ lang: 'bash', options: { name: 'step-1' }, code: '' })
})

test('note refs: Name when the header names the cell, else the block id', () => {
  const named = cellOf('bash', '#| name: preview\nls') as CellSpec
  const unnamed = cellOf('bash', '#| label: List\nls') as CellSpec
  expect(noteRef('doc1', named, 'blk1')).toEqual({ type: 'note', doc: 'doc1', key: { kind: 'name', value: 'preview' } })
  expect(noteRef('doc1', unnamed, 'blk1')).toEqual({ type: 'note', doc: 'doc1', key: { kind: 'block', value: 'blk1' } })
})

test('run-line and run-selection map BlockNote offsets to code lines and UTF-8 byte ranges (header excluded)', () => {
  const text = '#| name: x\n#| cwd: /tmp\necho é\nls -la\npwd'
  const spec = cellOf('bash', text) as CellSpec
  const code = text.indexOf('echo')
  expect(modeOf(text, spec, code, code)).toEqual({ type: 'line', line: 1 })
  expect(modeOf(text, spec, text.indexOf('pwd') + 1, text.indexOf('pwd') + 1)).toEqual({ type: 'line', line: 3 })
  // Caret on a header line: nothing to run.
  expect(modeOf(text, spec, 3, 3)).toBeNull()
  // `é` is two UTF-8 bytes: "echo é\n" = 8 bytes, so `ls` starts at byte 8.
  expect(modeOf(text, spec, text.indexOf('ls'), text.indexOf('ls') + 2)).toEqual({ type: 'selection', from: 8, to: 10 })
  // A selection starting in the header is clipped to the code; backwards selections count the same.
  expect(modeOf(text, spec, text.indexOf('ls') + 2, 2)).toEqual({ type: 'selection', from: 0, to: 10 })
  expect(modeOf(text, spec, 1, 4)).toBeNull()
})

test('member review diff, member names, python header rewrite, first defaults block', () => {
  expect(lineDiff('aws iam list-access-keys\nexit 0', 'aws iam create-access-key\naws iam list-access-keys\nexit 0')).toEqual([
    { op: '+', line: 'aws iam create-access-key' },
    { op: ' ', line: 'aws iam list-access-keys' },
    { op: ' ', line: 'exit 0' },
  ])
  expect(lineDiff('a\nb', 'a\nc')).toEqual([
    { op: ' ', line: 'a' },
    { op: '-', line: 'b' },
    { op: '+', line: 'c' },
  ])
  expect(memberName('AbCdEf12', [{ pub: 'AbCdEf12xyz', name: 'Alice' }])).toBe('Alice')
  expect(memberName('ZZZZZZZZ', [{ pub: 'AbCdEf12xyz', name: 'Alice' }])).toBe('ZZZZZZZZ')
  expect(withPython('#| name: t\n#| python: auto\nimport os', '.venv')).toBe('#| name: t\n#| python: .venv\nimport os')
  expect(withPython('#| name: t\nimport os', 'monty')).toBe('#| name: t\n#| python: monty\nimport os')
  const blocks = [
    { id: 'a', type: 'paragraph' },
    { id: 'b', type: 'codeBlock', props: { language: 'cells' } },
    { id: 'c', type: 'codeBlock', props: { language: 'cells' } },
  ]
  expect(firstDefaults(blocks, 'b')).toBe(true)
  expect(firstDefaults(blocks, 'c')).toBe(false)
})
