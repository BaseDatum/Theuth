import { expect, test } from 'vitest'
import { CODE_BLOCK_OPTIONS, diagramOf } from './codeLanguages'

test('language table: default text; the usual languages, mermaid (mmd) and json-render (jsonrender) present', () => {
  const langs = CODE_BLOCK_OPTIONS.supportedLanguages
  expect(CODE_BLOCK_OPTIONS.defaultLanguage).toBe('text')
  for (const id of ['text', 'typescript', 'rust', 'python', 'json', 'yaml', 'shellscript', 'mermaid', 'json-render']) expect(langs).toHaveProperty(id)
  expect(langs.mermaid.aliases).toContain('mmd')
  expect(langs['json-render'].aliases).toContain('jsonrender')
})

test('diagramOf: ids and aliases, any case; code languages and the default are not diagrams', () => {
  expect(diagramOf('mermaid')).toBe('mermaid')
  expect(diagramOf(' MMD ')).toBe('mermaid')
  expect(diagramOf('json-render')).toBe('json-render')
  expect(diagramOf('jsonrender')).toBe('json-render')
  for (const l of ['text', 'json', 'rust', '', 'mermaidx']) expect(diagramOf(l)).toBeNull()
})

test('CODE_BLOCK_OPTIONS carries createHighlighter: the github light and dark themes BlockNote picks by name', async () => {
  const h = await CODE_BLOCK_OPTIONS.createHighlighter()
  // BlockNote (`@blocknote/core` SyntaxHighlighting) highlights only with a /light/ and a /dark/ theme loaded.
  expect(h.getLoadedThemes().sort()).toEqual(['github-dark', 'github-light'])
})
