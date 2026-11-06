/** The code block's language table (59 §9.2 as-built), React-free so vitest can run it in node. */
import { codeBlockOptions } from '@blocknote/code-block'
import type { SyntaxHighlightingOptions } from '@blocknote/core'
import { loadHighlighter } from '@/lib/shiki'

/** A language the editor renders as a live surface instead of code (`CodeBlock.tsx`). */
export type Diagram = 'mermaid' | 'json-render'

/**
 * `@blocknote/code-block`'s table (the usual languages, `mermaid` with alias `mmd`) plus `json-render` (30 §4; no
 * grammar, so it stays unhighlighted). Default `text`. The ```` ```lang ```` input rule resolves aliases through this;
 * markdown/HTML import keeps the fence's info string verbatim. `createHighlighter` is the app's shiki (`lib/shiki.ts`,
 * the chat cells' and message fences' grammars and github themes), for `SyntaxHighlightingExtension` (`Editor.tsx`).
 */
export const CODE_BLOCK_OPTIONS = {
  defaultLanguage: 'text',
  supportedLanguages: {
    ...codeBlockOptions.supportedLanguages,
    'json-render': { name: 'json-render', aliases: ['json-render', 'jsonrender'] },
  },
  // BlockNote types it with shiki 4's types; it calls only `getLoadedLanguages`/`loadLanguage`/`getLoadedThemes`/
  // `codeToTokens`, which shiki 3 has alike.
  createHighlighter: loadHighlighter as SyntaxHighlightingOptions['createHighlighter'],
}

/** The diagram a language tag names, aliases included (`mmd`, `jsonrender`); `null` for code. */
export function diagramOf(language: string): Diagram | null {
  const l = language.trim().toLowerCase()
  for (const id of ['mermaid', 'json-render'] as const) {
    const { aliases } = CODE_BLOCK_OPTIONS.supportedLanguages[id]
    if (l === id || aliases.includes(l)) return id
  }
  return null
}
