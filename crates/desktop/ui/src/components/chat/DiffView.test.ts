import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { beforeEach, expect, test, vi } from 'vitest'
import { pairsDiff, parseUnified, toolDiff, wordChanges } from '@/lib/editDiff'
import { disclosureKey, useToolRuns } from '@/lib/tool-runs'
import type { Entry } from '@/store/sessions'
import { DiffStats, DiffView } from './DiffView'
import { MessageTool } from './MessageTool'

vi.mock('@tanstack/react-router', () => ({ Link: ({ children }: { children: unknown }) => children }))
beforeEach(() => useToolRuns.setState({ disclosed: {} }))
const patch = '--- a/src/a.ts\n+++ b/src/a.ts\n@@ -4,2 +4,2 @@ function a\n keep\n-old value\n+new value\n@@ -20 +20,2 @@\n-last\n+first\n+last\n\\ No newline at end of file\n--- a/b.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-gone\n'
const result = (data: unknown) => ({ output: JSON.stringify(data), isError: false, files: [] })
const carrier = (data: unknown) => ({ output: `receipt\n<theuth-diff-result>\n${JSON.stringify(data)}\n</theuth-diff-result>`, isError: false, files: [] })

test('unified files/hunks retain positions and no-newline notes without counting metadata', () => {
  const model = parseUnified(patch)
  expect(model.files.map((f) => f.path)).toEqual(['src/a.ts', 'b.txt'])
  expect([model.added, model.removed]).toEqual([3, 3])
  expect(model.files[0]!.rows).toContainEqual({ kind: 'line', text: 'new value', op: '+', old: undefined, next: 5 })
  expect(model.files[0]!.rows).toContainEqual({ kind: 'line', text: 'last', op: '+', old: undefined, next: 21 })
  expect(model.files[0]!.rows.at(-1)).toEqual({ kind: 'note', text: '\\ No newline at end of file' })
  expect(parseUnified('--- a/f\n+++ b/f\n@@ -1 +1 @@\n--- literal\n+++ literal\n').files).toHaveLength(1)
})

test('argument pairs compute additions, deletions, context and empty files without phantom lines', () => {
  const model = pairsDiff([{ path: 'f', old: 'a\nb\n', new: 'a\nB\nc\n' }])
  expect([model.added, model.removed]).toEqual([2, 1])
  expect(model.files[0]!.rows[1]).toMatchObject({ text: 'a', old: 1, next: 1, op: ' ' })
  expect(pairsDiff([{ path: 'new', old: '', new: 'hello\n' }])).toMatchObject({ added: 1, removed: 0 })
  expect(pairsDiff([{ path: 'empty', old: '', new: '' }])).toMatchObject({ added: 0, removed: 0 })
  expect(pairsDiff([{ path: 'newline', old: 'same', new: 'same\n' }])).toMatchObject({ added: 1, removed: 1 })
  const large = pairsDiff([{ path: 'large', old: 'a\n'.repeat(5000), new: 'b\n'.repeat(5000) }])
  expect([large.added, large.removed]).toEqual([5000, 5000])
})

test('edit lists, all, and range arguments use result diffs before attempted arguments', () => {
  expect(toolDiff('edit', { path: 'f', edits: [{ old: 'a', new: 'b' }, { old: 'c', new: 'd' }] })).toMatchObject({ added: 2, removed: 2 })
  expect(toolDiff('edit', { path: 'f', old: 'bad guess', new: 'wrong', all: true }, carrier({ diff: patch, added: 12, removed: 9 }))).toMatchObject({ added: 12, removed: 9 })
  const range = { path: 'f', range: 'L3-L4@abcd1234', new: 'replacement\n' }
  expect(toolDiff('edit', range)).toMatchObject({ added: 1, removed: 0 })
  const snapshot = toolDiff('edit', range, result({ snapshot: { text: 'a\nb\nc\nd\ne\n' } }))!
  expect(snapshot).toMatchObject({ added: 1, removed: 2 })
  expect(snapshot.files[0]!.rows[1]).toMatchObject({ old: 3, text: 'c' })
})

test('write_file creation is all additions; overwrite and no-op honor the result', () => {
  expect(toolDiff('write_file', { path: 'new', content: 'one\ntwo\n' }, result({ path: 'new', created: true, diff: '', added: 0, removed: 0 }))).toMatchObject({ added: 2, removed: 0 })
  expect(toolDiff('write_file', { path: 'f', content: 'not the diff' }, result({ diff: patch, created: false }))).toMatchObject({ added: 3, removed: 3 })
  expect(toolDiff('write_file', { path: 'f', content: 'same' }, result({ diff: '', created: false, added: 0, removed: 0 }))).toMatchObject({ added: 0, removed: 0 })
})

test.each(['move_text', 'copy_text', 'indent', 'edit'])('%s reads per-file result diffs including cross-file moves', (name) => {
  const model = toolDiff(name, {}, carrier({ files: [{ path: 'f', diff: patch }] }))!
  expect(model).toMatchObject({ added: 3, removed: 3, path: 'src/a.ts → b.txt' })
  expect(toolDiff(name, {}, carrier({ files: [{ path: 'unchanged.ts', diff: '' }] }))).toMatchObject({ path: 'unchanged.ts', added: 0, removed: 0 })
})

test('ACP marked unified carrier works for arbitrary tool titles and remains behind native/argument sources', () => {
  const acp = { output: `Edited files\n<theuth-acp-diff>\n${patch}\n</theuth-acp-diff>` }
  expect(toolDiff('Update sources', { kind: 'edit' }, acp)).toMatchObject({ added: 3, removed: 3 })
  expect(toolDiff('edit', { path: 'f', old: 'a', new: 'b' }, acp)).toMatchObject({ added: 1, removed: 1 })
  expect(toolDiff('Update sources', {}, acp)?.files).toHaveLength(2)
  expect(toolDiff('Update sources', {}, { output: acp.output + acp.output })?.files).toHaveLength(4)
  expect(toolDiff('shell', { command: 'echo hello' }, { output: patch })).toBeNull()
})

test('paired word changes mark individual identifiers, not unchanged punctuation or HTML', () => {
  const [old, next] = wordChanges('const first = second;', 'const newer = other;')
  expect(old.filter((p) => p.changed).map((p) => p.text)).toEqual(['first', 'second'])
  expect(next.filter((p) => p.changed).map((p) => p.text)).toEqual(['newer', 'other'])
  const html = renderToStaticMarkup(createElement(DiffView, { disclosure: 'c1', pairs: [{ path: 'f', old: '<script>old</script>', new: '<script>new</script>' }] }))
  expect(html).toContain('<ins ')
  expect(html).toContain('<del ')
  expect(html).toContain('&lt;script&gt;')
  expect(html).not.toContain('<script>')
})

test('DiffView caps rows across files and offers Show all without dropping the total counts', () => {
  const model = pairsDiff([{ path: 'large', old: '', new: Array.from({ length: 450 }, (_, i) => `line-${i}`).join('\n') + '\n' }])
  const html = renderToStaticMarkup(createElement(DiffView, { model, disclosure: 'c2' }))
  expect(html.match(/<tr/g)).toHaveLength(400)
  expect(html).toContain('Show all 451 lines')
  expect(html).not.toContain('line-449')
  expect(renderToStaticMarkup(createElement(DiffStats, model))).toContain('450 added, 0 removed')
})

test.each([false, true])('collapsed edit card includes path/counts; expanded child=%s uses DiffView and keeps disclosure state', (child) => {
  const tool: Extract<Entry, { kind: 'tool' }> = { id: 'e', kind: 'tool', callId: child ? 'p.1' : 'c', name: 'edit', args: { path: 'f', old: 'old', new: 'new' }, child, pending: false, result: carrier({ diff: patch, added: 8, removed: 2 }) }
  const render = () => renderToStaticMarkup(createElement(MessageTool, { tool, session: 's' }))
  const collapsed = render()
  expect(collapsed).toContain('aria-expanded="false"')
  expect(collapsed).toContain('src/a.ts → b.txt')
  expect(collapsed).toContain('8 added, 2 removed')
  expect(collapsed).not.toContain('aria-label="File diff"')
  useToolRuns.setState({ disclosed: { [disclosureKey('tool', tool.callId)]: true } })
  const expanded = render()
  expect(expanded).toContain('aria-label="File diff"')
  expect(expanded).toContain('aria-expanded="true"')
  expect(expanded).not.toContain('&lt;theuth-diff-result&gt;')
})
