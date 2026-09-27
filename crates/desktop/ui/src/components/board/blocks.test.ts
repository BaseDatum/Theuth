import { readFileSync } from 'node:fs'
import { expect, test } from 'vitest'
import { type DocBlock, classifyCode, codeAnchor, excerptsOf, plainText } from './blocks'
import { coveringExcerpt } from './SidePane'

/** The fixture board's ```json-render fences, as the `codeBlock`s BlockNote would hold them. */
const FIXTURE = readFileSync(new URL('./fixtures/surface-fence-parsing.md', import.meta.url), 'utf8')
const fences = [...FIXTURE.matchAll(/```json-render\n([\s\S]*?)\n```/g)].map((m) => m[1]!)
const codeBlock = (id: string, language: string, text: string): DocBlock =>
  ({ id, type: 'codeBlock', props: { language }, content: [{ type: 'text', text, styles: {} }], children: [] }) as unknown as DocBlock

test('the fixture board classifies: Graph and Sequence take the wide column, Excerpts become code windows, Checklist stays prose width', () => {
  const kinds = fences.map((f, i) => classifyCode(codeBlock(`b${i}`, 'json-render', f) as Extract<DocBlock, { type: 'codeBlock' }>).kind)
  expect(kinds).toEqual(['diagram', 'diagram', 'excerpt', 'excerpt', 'excerpt', 'surface'])
  const excerpts = excerptsOf(fences.map((f, i) => codeBlock(`b${i}`, 'json-render', f)))
  expect(excerpts.map((e) => e.anchor.lines)).toEqual([
    [74, 87],
    [89, 95],
    [38, 59],
  ])
  expect(excerpts[0]!.blockId).toBe('b2')
})

test('mermaid, plain code and a broken spec each map to their own kind', () => {
  expect(classifyCode(codeBlock('m', 'mermaid', 'flowchart LR\n A-->B') as never)).toEqual({ kind: 'mermaid', code: 'flowchart LR\n A-->B' })
  expect(classifyCode(codeBlock('c', 'rust', 'fn x() {}') as never)).toEqual({ kind: 'code', lang: 'rust', text: 'fn x() {}' })
  const bad = classifyCode(codeBlock('x', 'json-render', '{"root":"r","elements":{"r":{"type":"Excerpt","props":{"path":"../x","text":"a"}}}}') as never)
  expect(bad.kind).toBe('invalid')
})

test('code: hrefs resolve to anchors; other hrefs and bad grammar do not', () => {
  expect(codeAnchor('code:crates/surface/src/fences.rs#L83')).toEqual({ kind: 'code', path: 'crates/surface/src/fences.rs', lines: [83, 83] })
  expect(codeAnchor('code:crates/surface/src/fences.rs')).toEqual({ kind: 'code', path: 'crates/surface/src/fences.rs' })
  expect(codeAnchor('https://example.com')).toBeNull()
  expect(codeAnchor('code:../x')).toBeNull()
  expect(codeAnchor('code:a.rs#L9-L3')).toBeNull()
})

test('the side pane picks the excerpt containing the pin, or the file’s first for a bare path', () => {
  const excerpts = excerptsOf(fences.map((f, i) => codeBlock(`b${i}`, 'json-render', f)))
  expect(coveringExcerpt(excerpts, { kind: 'code', path: 'crates/surface/src/fences.rs', lines: [83, 83] })?.blockId).toBe('b2')
  expect(coveringExcerpt(excerpts, { kind: 'code', path: 'crates/surface/src/fences.rs', lines: [90, 94] })?.blockId).toBe('b3')
  expect(coveringExcerpt(excerpts, { kind: 'code', path: 'crates/surface/src/fences.rs', lines: [24, 65] })).toBeNull() // spills past L38–L59
  expect(coveringExcerpt(excerpts, { kind: 'code', path: 'crates/surface/src/fences.rs' })?.blockId).toBe('b2')
  expect(coveringExcerpt(excerpts, { kind: 'code', path: 'crates/surface/src/lib.rs', lines: [1, 1] })).toBeNull()
  expect(coveringExcerpt(excerpts, { kind: 'block', id: 'z' })).toBeNull()
})

test('plainText flattens runs and links into the claim line', () => {
  expect(
    plainText([
      { type: 'text', text: 'The ', styles: {} },
      { type: 'link', href: 'code:a.rs#L1', content: [{ type: 'text', text: 'parse()', styles: {} }] },
      { type: 'text', text: ' call.', styles: {} },
    ] as never),
  ).toBe('The parse() call.')
})
