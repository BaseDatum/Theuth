// Design 65 §4.2, §3.4: the palette a diagram paints with, read off the shadcn variables in `styles.css` so mermaid's
// `themeVariables` and the Graph svg's inline `fill`/`stroke` (which must survive export, where no stylesheet
// follows) track the theme without a second colour table. Read once per theme change, not per render.

export interface DiagramTokens {
  background: string
  foreground: string
  primary: string
  muted: string
  mutedForeground: string
  border: string
  card: string
  accent: string
  success: string
  danger: string
  font: string
  /** 123 §5.1: six categorical series colours (`--chart-1..6` when the theme defines them), charts and legends. */
  series: [string, string, string, string, string, string]
}

/** Current values of the CSS custom properties, resolved through their `var()` chains. */
export function diagramTokens(): DiagramTokens {
  const s = getComputedStyle(document.documentElement)
  const v = (name: string, fallback: string) => s.getPropertyValue(name).trim() || fallback
  return {
    background: v('--background', '#e8dcc8'),
    foreground: v('--foreground', '#1a1612'),
    primary: v('--primary', '#1a1612'),
    muted: v('--muted', '#d4c4a8'),
    mutedForeground: v('--muted-foreground', '#6b5e4f'),
    border: v('--border', '#d4c4a8'),
    card: v('--card', '#f3eadf'),
    accent: v('--fg-accent', '#b4531e'),
    success: v('--status-success-fg', '#2f5d4a'),
    danger: v('--status-danger-fg', '#9b2d1f'),
    font: v('--font-sans', 'ui-sans-serif, system-ui, sans-serif'),
    series: [
      v('--chart-1', '#b4531e'),
      v('--chart-2', '#2f5d4a'),
      v('--chart-3', '#3b5b8c'),
      v('--chart-4', '#8a6d1f'),
      v('--chart-5', '#7a3e7a'),
      v('--chart-6', '#6b5e4f'),
    ],
  }
}

/**
 * The light palette (72 §3: print exports are light whatever the app shows). Read by lifting `.dark` off the root for
 * one synchronous style resolution — no paint happens in between, so nothing flashes.
 */
export function lightTokens(): DiagramTokens {
  const root = document.documentElement
  if (!root.classList.contains('dark')) return diagramTokens()
  root.classList.remove('dark')
  try {
    return diagramTokens()
  } finally {
    root.classList.add('dark')
  }
}
