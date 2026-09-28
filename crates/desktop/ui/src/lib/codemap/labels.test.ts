import { expect, test } from 'vitest'
import { anchorOf, BADGE, GRAPH_STATE, kindGlyph, shortLabels } from './labels'

test('shortLabels: two tools.rs get the shortest distinguishing suffix; unique basenames stay basenames', () => {
  const m = shortLabels(['crates/scrape/src/tools.rs', 'crates/knowledge/src/tools.rs', 'crates/agent/src/loop_.rs'])
  expect(m.get('crates/scrape/src/tools.rs')).toBe('scrape/src/tools.rs')
  expect(m.get('crates/knowledge/src/tools.rs')).toBe('knowledge/src/tools.rs')
  expect(m.get('crates/agent/src/loop_.rs')).toBe('loop_.rs')
  expect(shortLabels(['a/x.rs', 'a/x.rs']).get('a/x.rs')).toBe('x.rs')
})

test('kindGlyph: five glyphs, unknown kinds get none', () => {
  expect(['function', 'method'].map(kindGlyph)).toEqual(['ƒ', 'ƒ'])
  expect(['class', 'interface', 'type', 'struct', 'trait', 'enum'].map(kindGlyph)).toEqual(['◇', '◇', '◇', '◇', '◇', '◇'])
  expect(['module', 'namespace', 'file'].map(kindGlyph)).toEqual(['≡', '≡', '≡'])
  expect(kindGlyph('macro')).toBe('#')
  expect(['constant', 'const', 'static', 'variable'].map(kindGlyph)).toEqual(['=', '=', '=', '='])
  expect(kindGlyph('impl')).toBe('')
})

test('anchorOf: range, single line (b === a or missing), bare path', () => {
  expect(anchorOf('a/b.rs', 10, 24)).toBe('a/b.rs#L10-L24')
  expect(anchorOf('a/b.rs', 10, 10)).toBe('a/b.rs#L10')
  expect(anchorOf('a/b.rs', 10)).toBe('a/b.rs#L10')
  expect(anchorOf('a/b.rs', null, 5)).toBe('a/b.rs')
  expect(anchorOf('a/b.rs')).toBe('a/b.rs')
})

test('GRAPH_STATE covers the five colour states; every badge word is its state name', () => {
  expect(GRAPH_STATE).toEqual({ changed: 'danger', impacted: 'active', transitive: 'muted', test: 'done', stale: 'default' })
  for (const [state, word] of Object.entries(BADGE)) expect(word).toBe(state)
  expect(Object.keys(BADGE).sort()).toEqual(Object.keys(GRAPH_STATE).sort())
})
