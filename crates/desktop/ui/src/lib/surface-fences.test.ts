import { expect, test } from 'vitest'
import { surfaceFences } from './surface-fences'

// 197 §3's rule; the engine's detector runs the same cases (desktop and engine must agree on every fence index).
const langs = (text: string) => surfaceFences(text).map((f) => f.language)

test('the three surface languages count, in order; every other fence is ignored', () => {
  const text = [
    'intro',
    '```json-render',
    '{"root":"a"}',
    '```',
    '```rust',
    'fn main() {}',
    '```',
    '~~~mermaid',
    'sequenceDiagram',
    '~~~',
    '```openui',
    'root = Card()',
    '```',
    'outro',
  ].join('\n')
  expect(surfaceFences(text)).toEqual([
    { language: 'json-render', body: '{"root":"a"}' },
    { language: 'mermaid', body: 'sequenceDiagram' },
    { language: 'openui', body: 'root = Card()' },
  ])
})

test('the info string is exactly the language, trailing whitespace allowed; anything else is not a surface fence', () => {
  expect(langs('```mermaid  \nx\n```')).toEqual(['mermaid'])
  expect(langs('``` mermaid\nx\n```')).toEqual([])
  expect(langs('```mermaid title\nx\n```')).toEqual([])
  expect(langs('```json\n{}\n```')).toEqual([])
  expect(langs('```JSON-RENDER\n{}\n```')).toEqual([])
  expect(langs('``mermaid\nx\n``')).toEqual([])
  // No indentation: a fence inside a list item is not counted (the transcript then offers no control).
  expect(langs('- item\n  ```mermaid\n  x\n  ```')).toEqual([])
})

test('a fence closes only on a line of the same character, at least as long', () => {
  const [f] = surfaceFences('````json-render\n```\n~~~~\n`````\nafter')
  expect(f).toEqual({ language: 'json-render', body: '```\n~~~~' })
  expect(surfaceFences('~~~mermaid\n```\n~~~ \n')).toEqual([{ language: 'mermaid', body: '```' }])
  // A closer with text after it is body.
  expect(surfaceFences('```mermaid\n``` x\n```')).toEqual([{ language: 'mermaid', body: '``` x' }])
})

test('an opener inside an open surface fence is body, and an unclosed fence runs to the end', () => {
  expect(surfaceFences('```openui\n```mermaid\nx')).toEqual([{ language: 'openui', body: '```mermaid\nx' }])
  expect(surfaceFences('text\n```json-render\n{"root":')).toEqual([{ language: 'json-render', body: '{"root":' }])
})

test('openers inside other fences still count (other fences are not tracked)', () => {
  expect(langs('````markdown\n```mermaid\nx\n```\n````')).toEqual(['mermaid'])
})
