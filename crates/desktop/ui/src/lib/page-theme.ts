// Design 120 §7: theuth tokens → T3 Code's forty `html_render` variables, so a page written for T3 renders unchanged.
// Resolved off `getComputedStyle(document.documentElement)` like `diagramTokens()` (the `var()` chains collapse to
// colour strings; the page never sees theuth's token names), re-resolved on the `<html class>` flip.

import { useMemo, useSyncExternalStore } from 'react'
import { subscribeDark } from './theme'

export type Appearance = 'dark' | 'light'
export type PageTheme = { appearance: Appearance; variables: Record<string, string> }

/** The literal `"SF Mono"…` list (T3 `htmlRender.ts:157`): some engines alias `ui-monospace` to the UI font. */
const FONT_MONO = '"SF Mono", "SFMono-Regular", Menlo, Consolas, "Liberation Mono", monospace'
const FONT_SANS = 'ui-sans-serif, system-ui, sans-serif'

/** T3 name → [theuth token to read (`null` = literal only), light literal, dark literal] (§7.1 table; literals from `styles.css`). */
const SOURCES: Record<string, [token: string | null, light: string, dark: string]> = {
  '--background': ['--background', '#e8dcc8', '#12100e'],
  '--foreground': ['--foreground', '#1a1612', '#e8dcc8'],
  '--muted': ['--muted', '#d4c4a8', '#2a241c'],
  '--muted-foreground': ['--muted-foreground', '#6b5e4f', '#c9b89a'],
  '--card': ['--card', '#f3eadf', '#1c1915'],
  '--card-foreground': ['--card-foreground', '#1a1612', '#e8dcc8'],
  '--popover': ['--popover', '#f3eadf', '#1c1915'],
  '--popover-foreground': ['--popover-foreground', '#1a1612', '#e8dcc8'],
  '--secondary': ['--secondary', '#d4c4a8', '#2a241c'],
  '--secondary-foreground': ['--secondary-foreground', '#1a1612', '#e8dcc8'],
  '--border': ['--border', '#d4c4a8', '#2a241c'],
  '--input': ['--input', '#d4c4a8', '#2a241c'],
  '--ring': ['--ring', '#1d5f82', '#52a2cc'],
  '--primary': ['--primary', '#1a1612', '#e8dcc8'],
  '--primary-foreground': ['--primary-foreground', '#e8dcc8', '#12100e'],
  // T3's `--accent` is the brand accent, not shadcn's neutral hover wash: the Egyptian blue token, not `--accent`.
  '--accent': ['--fg-accent', '#1d5f82', '#52a2cc'],
  '--accent-foreground': ['--fg-inverse', '#e8dcc8', '#12100e'],
  '--accent-surface': ['--status-warning-bg', '#1d5f821f', '#52a2cc26'],
  '--accent-surface-foreground': ['--fg-accent-hover', '#154863', '#408cb5'],
  '--destructive': ['--status-danger-fg', '#8f2f1f', '#8f2f1f'],
  '--destructive-foreground': ['--fg-inverse', '#e8dcc8', '#12100e'],
  '--destructive-surface': ['--status-danger-bg', '#8f2f1f1f', '#8f2f1f26'],
  '--warning': ['--status-warning-fg', '#1d5f82', '#52a2cc'],
  '--warning-foreground': ['--fg-inverse', '#e8dcc8', '#12100e'],
  '--warning-surface': ['--status-warning-bg', '#1d5f821f', '#52a2cc26'],
  '--success': ['--status-success-fg', '#265c47', '#4f9c7c'],
  '--success-foreground': ['--fg-inverse', '#e8dcc8', '#12100e'],
  '--info': ['--status-info-fg', '#3d6b66', '#5b8f8a'],
  '--info-foreground': ['--fg-inverse', '#e8dcc8', '#12100e'],
  '--code-background': ['--bg-layer-3', '#c9b48a', '#3a3228'],
  '--code-foreground': ['--fg-primary', '#1a1612', '#e8dcc8'],
  '--chart-1': ['--fg-accent', '#1d5f82', '#52a2cc'],
  '--chart-2': ['--project-faience', '#3d6b66', '#5b8f8a'],
  '--chart-3': ['--project-malachite', '#265c47', '#4f9c7c'],
  '--chart-4': ['--project-lapis', '#3c5a86', '#6f8fc0'],
  '--chart-5': ['--project-carnelian', '#8f2f1f', '#8f2f1f'],
  '--chart-6': ['--project-plum', '#7d3f5c', '#b0708f'],
  '--radius': ['--radius', '0.75rem', '0.75rem'],
  '--font-sans': ['--font-sans', FONT_SANS, FONT_SANS],
  '--font-mono': [null, FONT_MONO, FONT_MONO],
}

/** The forty names, in T3's order (`htmlRender.ts:209-244`). */
export const PAGE_VARIABLES: readonly string[] = Object.keys(SOURCES)

/** The literal palette for `appearance`: the §7.4 defaults baked into the bootstrap stylesheet. */
export function pageDefaults(appearance: Appearance): Record<string, string> {
  const out: Record<string, string> = {}
  for (const [name, [, light, dark]] of Object.entries(SOURCES)) out[name] = appearance === 'dark' ? dark : light
  return out
}

const resolve = (appearance: Appearance): PageTheme => {
  const s = getComputedStyle(document.documentElement)
  const variables: Record<string, string> = {}
  for (const [name, [token, light, dark]] of Object.entries(SOURCES)) {
    const fallback = appearance === 'dark' ? dark : light
    variables[name] = (token && s.getPropertyValue(token).trim()) || fallback
  }
  return { appearance, variables }
}

const isDark = () => document.documentElement.classList.contains('dark')

/**
 * The theme for `appearance` whatever the app shows: `lightTokens()`'s class-lift trick (`diagram-theme.ts:42-51`) in
 * both directions — toggle `.dark` for one synchronous style resolution; no paint happens in between.
 */
export function pageThemeFor(appearance: Appearance): PageTheme {
  const root = document.documentElement
  const dark = isDark()
  if (dark === (appearance === 'dark')) return resolve(appearance)
  root.classList.toggle('dark', !dark)
  try {
    return resolve(appearance)
  } finally {
    root.classList.toggle('dark', dark)
  }
}

/** The app's current page theme; a new object on every `<html class>` flip. */
export function useHtmlPageTheme(): PageTheme {
  const dark = useSyncExternalStore(subscribeDark, isDark, () => false)
  return useMemo(() => resolve(dark ? 'dark' : 'light'), [dark])
}
