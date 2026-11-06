/**
 * One shiki highlighter for cell code in notes (BlockNote's `SyntaxHighlightingExtension`) and chat cells
 * (`cells/shiki-marks.ts`): the themes, grammars and regex engine of the chat's message fences (`@streamdown/code`),
 * so a cell looks like a fence. Grammars and themes are lazy chunks loaded on first use, per language; the shiki core
 * itself is not (`@streamdown/code` imports it statically for the message fences).
 */
import type { Highlighter, ThemedToken } from 'shiki'

export const SHIKI_THEMES = { light: 'github-light', dark: 'github-dark' } as const

/** Fence/block language → shiki id for every cell `Lang` and the forms notes store (BlockNote saves bash as `shellscript`). */
const ALIASES: Record<string, string> = {
  bash: 'shellscript',
  zsh: 'shellscript',
  sh: 'shellscript',
  shellscript: 'shellscript',
  shell: 'shellscript',
  pwsh: 'powershell',
  powershell: 'powershell',
  ps1: 'powershell',
  python: 'python',
  py: 'python',
  javascript: 'javascript',
  js: 'javascript',
  typescript: 'typescript',
  ts: 'typescript',
  // shiki's plain-text ids that never appear in `getLoadedLanguages`: BlockNote would load them again on every refresh.
  // `text` is one BlockNote skips.
  ansi: 'text',
  plain: 'text',
  plaintext: 'text',
  txt: 'text',
}

/** The shiki language for `language`; anything else passes through (BlockNote's table already uses shiki ids). */
export const shikiLang = (language: string | null | undefined) => {
  const l = (language ?? '').trim().toLowerCase()
  return Object.hasOwn(ALIASES, l) ? (ALIASES[l] as string) : l
}

const load = async (): Promise<Highlighter> => {
  const [{ createHighlighter }, { createJavaScriptRegexEngine }] = await Promise.all([import('shiki'), import('shiki/engine/javascript')])
  const h = await createHighlighter({ themes: Object.values(SHIKI_THEMES), langs: [], engine: createJavaScriptRegexEngine({ forgiving: true }) })
  // BlockNote calls this inside ProseMirror's decoration pass, where shiki's synchronous throw for a language it has no
  // grammar for (`cells`, `json-render`, a typo) aborts the pass: no block in the note gets highlighted. A rejection
  // is what BlockNote expects; it then leaves that language plain.
  const loadLanguage = h.loadLanguage
  h.loadLanguage = async (...langs) => loadLanguage(...langs)
  return h
}
let highlighter: Promise<Highlighter> | undefined
/** The shared highlighter; a failed load (a chunk that did not fetch) is retried by the next caller. */
export const loadHighlighter = () =>
  (highlighter ??= load().catch((e: unknown) => {
    highlighter = undefined
    throw e
  }))

/** `code`'s tokens with both themes as `--shiki-light`/`--shiki-dark` (`htmlStyle`); the `.shiki` rule in styles.css picks one. */
export async function tokensOf(code: string, language: string): Promise<ThemedToken[][]> {
  const h = await loadHighlighter()
  const lang = shikiLang(language)
  await h.loadLanguage(lang as never)
  return h.codeToTokens(code, { lang: lang as never, themes: SHIKI_THEMES, defaultColor: false }).tokens
}
