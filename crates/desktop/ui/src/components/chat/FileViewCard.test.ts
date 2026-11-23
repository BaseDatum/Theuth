import { expect, test } from 'vitest'
import type { Entry } from '@/store/sessions'
import { diffCall } from './DiffViewCard'
import { fileCall } from './FileViewCard'

type Tool = Extract<Entry, { kind: 'tool' }>
const ID = '0123456789abcdef'
const tool = (name: string, over: Partial<Tool>): Tool => ({ id: 'e1', kind: 'tool', callId: 'c1', name, args: {}, pending: false, child: false, ...over })
const result = (output: string, isError = false) => ({ output, isError, files: [] })

test('a show_file receipt names the title, the tile id, the absolute path and the marked lines', () => {
  const head = `file view opened · main.rs · tile file:${ID}`
  const tail = '\n120 lines, 3400 bytes.\nThe tile follows the file as it changes on disk.'
  expect(fileCall(tool('show_file', { result: result(`${head}\npath: /repo/src/main.rs#L10-L20${tail}`) }))).toEqual({ title: 'main.rs', id: ID, path: '/repo/src/main.rs', line: 10, end_line: 20 })
  expect(fileCall(tool('show_file', { result: result(`${head}\npath: /repo/src/main.rs#L7${tail}`) }))).toEqual({ title: 'main.rs', id: ID, path: '/repo/src/main.rs', line: 7 })
  expect(fileCall(tool('show_file', { result: result(`${head}\npath: /repo/a b/#odd.rs${tail}`) }))).toEqual({ title: 'main.rs', id: ID, path: '/repo/a b/#odd.rs' })
})

test('a pending, failed or malformed show_file call has no id; its arguments name it', () => {
  expect(fileCall(tool('show_file', { pending: true, args: { path: 'src/lib.rs', line: 3 } }))).toEqual({ title: 'lib.rs', id: null, path: 'src/lib.rs', line: 3 })
  expect(fileCall(tool('show_file', { args: { path: 'x.rs', title: 'X' }, result: result('show_file: x.rs is binary', true) }))).toMatchObject({ title: 'X', id: null })
  // The id is exactly 16 lowercase hex.
  expect(fileCall(tool('show_file', { result: result('file view opened · t · tile file:ABC\npath: /a') })).id).toBeNull()
})

test('a show_diff receipt names the title, the tile id, the spec and the counts', () => {
  const spec = { mode: 'range', root: '/repo', a: 'a'.repeat(40), b: 'b'.repeat(40), three_dot: false, paths: ['src'] }
  const out = `diff view opened · main..HEAD · src · tile diff:${ID}\nspec: ${JSON.stringify(spec)}\n3 files changed, +40 −12.`
  expect(diffCall(tool('show_diff', { result: result(out) }))).toEqual({ title: 'main..HEAD · src', id: ID, spec, stats: { files: 3, added: 40, removed: 12 } })
  const none = `diff view opened · working tree · tile diff:${ID}\nspec: {"mode":"worktree","root":"/repo","a":null,"b":null,"three_dot":false,"paths":[]}\nno changes.`
  expect(diffCall(tool('show_diff', { result: result(none) }))).toMatchObject({ id: ID, stats: 'none' })
})

test('a pending, failed or unparsable show_diff call has no spec; its arguments name it', () => {
  expect(diffCall(tool('show_diff', { pending: true, args: { range: 'main..HEAD' } }))).toEqual({ title: 'main..HEAD', id: null, spec: null, stats: null })
  expect(diffCall(tool('show_diff', { pending: true, args: { file_a: '/x/a.txt', file_b: '/x/b.txt' } })).title).toBe('a.txt ↔ b.txt')
  expect(diffCall(tool('show_diff', { pending: true })).title).toBe('working tree')
  expect(diffCall(tool('show_diff', { result: result(`diff view opened · t · tile diff:${ID}\nspec: {not json}`) })).id).toBeNull()
  expect(diffCall(tool('show_diff', { result: result('show_diff: unknown revision nope', true) })).spec).toBeNull()
})
