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
  }
}
