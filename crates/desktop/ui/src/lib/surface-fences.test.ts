import { expect, test } from 'vitest'
import { surfaceFences } from './surface-fences'

// Ported from the engine's `surface_fence_scanner_rule` (crates/agent/src/context.rs) and its `reply()` fixture: the
// desktop and the engine must agree on every fence index.
const langs = (text: string) => surfaceFences(text).map((f) => f.language)

test('surface_fence_scanner_rule', () => {
  const t =
    '```not``` a fence\n  ```mermaid\nindented: content\n``` mermaid\nspaced: other fence\n```\na\n~~~~mermaid\ngraph TD\n~~~\nstill inside\n~~~~~\nb\n```json-render \r\n{}\r\n```\r\nc\n```bash\n```openui\n```\n```openui\nroot = X()\n'
  expect(surfaceFences(t)).toEqual([
    // A shorter run of the same character is content.
    { language: 'mermaid', body: 'graph TD\n~~~\nstill inside\n' },
    // Trailing whitespace after the info string and `\r\n` line ends.
    { language: 'json-render', body: '{}\r\n' },
    // Unclosed: to the end of the text.
    { language: 'openui', body: 'root = X()\n' },
  ])
})

test('two in one reply; none in plain text or inside other fences', () => {
  const reply =
    'The branch is in.\n```json-render\n{"root":"a"}\n```\nTwo things:\n```mermaid\nsequenceDiagram\n  A->>B: hi\n```\n````rust\n```mermaid\nnot a fence\n```\n````\nDone.'
  expect(langs(reply)).toEqual(['json-render', 'mermaid'])
  expect(surfaceFences('```rust\n```mermaid\n```\n')).toEqual([])
  expect(surfaceFences('no fences')).toEqual([])
})

test('only the exact surface languages count; any other opener is tracked and skipped whole', () => {
  expect(langs('```\n```mermaid\n```\n```mermaid\nx\n```')).toEqual(['mermaid'])
  expect(langs('~~~~md\n```mermaid\nx\n```\n~~~~\n```openui\ny\n```')).toEqual(['openui'])
  expect(langs('```JSON-RENDER\n{}\n```')).toEqual([])
  expect(langs('``mermaid\nx\n``')).toEqual([])
  // A closer with text after it is content.
  expect(surfaceFences('```mermaid\n``` x\n```')).toEqual([{ language: 'mermaid', body: '``` x\n' }])
})
