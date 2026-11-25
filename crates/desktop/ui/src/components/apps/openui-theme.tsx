import { ThemeProvider, createTheme } from '@openuidev/react-ui'
import type { ReactNode } from 'react'

/**
 * theuth's OpenUI skin (design 46): one deliberate mapping of this app's palette onto OpenUI's token surface, shared by
 * every mount — the inline fence (`chat/surface/OpenUIFence`) and the live app view (`AppHost`) cannot diverge. Every
 * value is a `var()` reference to a token `styles.css` declares on `:root`/`.dark` (or a `color-mix` wash of one, the
 * status-wash pattern — never a new hex), so a single object serves both modes: the tokens flip with `.dark` themselves,
 * and a fence snapshot's forced appearance holds because its frame re-declares them (`FenceSnapshot.themeVariables`).
 * Keys left unmapped keep OpenUI's per-mode defaults (spacing, text sizes, the variable `wght`-axis typography, the
 * subtle shadows).
 */
export const theuthOpenUITheme = createTheme({
  // Surfaces: the app canvas is the page the fence merges into (no card chrome); wells are the limestone/basalt and
  // papyrus/char washes theuth's own inputs and code wells use; everything elevated is the one raised surface, the
  // hairline borders doing the separating — not shadow tiers.
  background: 'var(--bg-page)',
  foreground: 'var(--bg-layer-1)',
  popoverBackground: 'var(--bg-elevated)',
  sunkLight: 'var(--bg-layer-2)',
  sunk: 'var(--bg-layer-2)',
  sunkDeep: 'var(--bg-layer-3)',
  elevatedLight: 'var(--bg-elevated)',
  elevated: 'var(--bg-elevated)',
  elevatedStrong: 'var(--bg-elevated)',
  elevatedIntense: 'var(--bg-elevated)',
  // The scrim `ui/dialog` uses (bg-black/50): a scrim, not a palette colour.
  overlay: 'rgb(0 0 0 / 50%)',
  highlightSubtle: 'var(--bg-layer-2)',
  highlight: 'var(--bg-layer-2)',
  highlightStrong: 'var(--bg-layer-3)',
  highlightIntense: 'var(--bg-layer-3)',
  // Ink inverted: soot under bone (light), bone under soot (dark).
  invertedBackground: 'var(--fg-primary)',

  // Status washes and texts: the minerals.
  infoBackground: 'var(--status-info-bg)',
  successBackground: 'var(--status-success-bg)',
  alertBackground: 'var(--status-warning-bg)',
  dangerBackground: 'var(--status-danger-bg)',
  // The palette has no purple or pink; the project hues' plum stands in for both (no component styles them today).
  purpleBackground: 'color-mix(in oklab, var(--project-plum) 12%, transparent)',
  pinkBackground: 'color-mix(in oklab, var(--project-plum) 12%, transparent)',
  textPinkPrimary: 'var(--project-plum)',
  textPinkInverted: 'var(--fg-inverse)',
  textPurplePrimary: 'var(--project-plum)',
  textPurpleInverted: 'var(--fg-inverse)',

  // Neutral text, alpha-tiered by theuth's own secondary/tertiary inks; links are the accent.
  textNeutralPrimary: 'var(--fg-primary)',
  textNeutralSecondary: 'var(--fg-secondary)',
  textNeutralTertiary: 'var(--fg-tertiary)',
  textNeutralLink: 'var(--fg-accent)',
  textBrand: 'var(--fg-accent)',
  textWhite: 'var(--fg-inverse)',
  textBlack: 'var(--fg-primary)',
  // On the Egyptian blue accent, the inverted ink reads best in both modes.
  textAccentPrimary: 'var(--fg-inverse)',
  textAccentSecondary: 'color-mix(in oklab, var(--fg-inverse) 60%, transparent)',
  textAccentTertiary: 'color-mix(in oklab, var(--fg-inverse) 30%, transparent)',
  textSuccessPrimary: 'var(--status-success-fg)',
  textSuccessInverted: 'var(--fg-inverse)',
  textAlertPrimary: 'var(--status-warning-fg)',
  textAlertInverted: 'var(--fg-inverse)',
  textDangerPrimary: 'var(--status-danger-fg)',
  textDangerSecondary: 'color-mix(in oklab, var(--status-danger-fg) 60%, transparent)',
  textDangerTertiary: 'color-mix(in oklab, var(--status-danger-fg) 40%, transparent)',
  textDangerInvertedPrimary: 'var(--fg-inverse)',
  textDangerInvertedSecondary: 'color-mix(in oklab, var(--fg-inverse) 60%, transparent)',
  textDangerInvertedTertiary: 'color-mix(in oklab, var(--fg-inverse) 40%, transparent)',
  textInfoPrimary: 'var(--status-info-fg)',
  textInfoInverted: 'var(--fg-inverse)',

  // Interactive: Egyptian blue is the accent (theuth's `--ring`, underlined links), carnelian the destructive.
  interactiveAccentDefault: 'var(--fg-accent)',
  interactiveAccentHover: 'var(--fg-accent-hover)',
  interactiveAccentPressed: 'var(--fg-accent-hover)',
  interactiveAccentDisabled: 'color-mix(in oklab, var(--fg-accent) 40%, transparent)',
  interactiveDestructiveDefault: 'var(--status-danger-bg)',
  interactiveDestructiveHover: 'color-mix(in oklab, var(--status-danger-fg) 16%, transparent)',
  interactiveDestructivePressed: 'color-mix(in oklab, var(--status-danger-fg) 24%, transparent)',
  interactiveDestructiveDisabled: 'var(--status-danger-bg)',
  interactiveDestructiveAccentDefault: 'var(--status-danger-fg)',
  interactiveDestructiveAccentHover: 'var(--status-danger-fg)',
  interactiveDestructiveAccentPressed: 'var(--status-danger-fg)',
  interactiveDestructiveAccentDisabled: 'color-mix(in oklab, var(--status-danger-fg) 40%, transparent)',

  // Hairline borders; emphasis is the border at alpha, as the status washes are.
  borderDefault: 'var(--border-subtle)',
  borderInteractive: 'var(--border-default)',
  borderInteractiveEmphasis: 'var(--border-strong)',
  borderInteractiveSelected: 'var(--fg-accent)',
  borderAccent: 'color-mix(in oklab, var(--fg-accent) 40%, transparent)',
  borderAccentEmphasis: 'color-mix(in oklab, var(--fg-accent) 70%, transparent)',
  borderInfo: 'color-mix(in oklab, var(--status-info-fg) 20%, transparent)',
  borderInfoEmphasis: 'var(--status-info-border)',
  borderAlert: 'color-mix(in oklab, var(--status-warning-fg) 20%, transparent)',
  borderAlertEmphasis: 'var(--status-warning-border)',
  borderSuccess: 'color-mix(in oklab, var(--status-success-fg) 20%, transparent)',
  borderSuccessEmphasis: 'var(--status-success-border)',
  borderDanger: 'color-mix(in oklab, var(--status-danger-fg) 20%, transparent)',
  borderDangerEmphasis: 'var(--status-danger-border)',

  // Charts: the shadcn `--chart-1..8` series in the same order (160 D2) — the project minerals, never the accent or
  // carnelian (warning / danger). `var()` composes with the two-level indirection the chart renderer builds
  // (`--color-<series>: var(--chart-3)`, then `fill: var(--color-<series>)`); the per-chart-type palettes fall back
  // to this one (`useChartPalette`).
  defaultChartPalette: [
    'var(--chart-1)',
    'var(--chart-2)',
    'var(--chart-3)',
    'var(--chart-4)',
    'var(--chart-5)',
    'var(--chart-6)',
    'var(--chart-7)',
    'var(--chart-8)',
  ],

  // Radii: theuth's scale around `--radius` (8/10/12/16), not OpenUI's 6–14 — controls at 8/10, the design radius 12,
  // cards 16. The unused tiers above `3xl` keep their defaults.
  radiusS: 'calc(var(--radius) - 4px)',
  radiusM: 'calc(var(--radius) - 2px)',
  radiusL: 'var(--radius)',
  radiusXl: 'var(--radius)',
  radius2xl: 'calc(var(--radius) + 4px)',
  radius3xl: 'calc(var(--radius) + 4px)',

  // OpenUI sets Inter through the variable `wght` axis (`font-variation-settings`, not whole-number weights); the
  // bundled variable face (`main.tsx`) is what makes that render, with no runtime font fetch. Code keeps OpenUI's
  // native mono stack.
  fontBody: '"Inter Variable", "Inter", sans-serif',
  fontHeading: '"Inter Variable", "Inter", sans-serif',
  fontLabel: '"Inter Variable", "Inter", sans-serif',
  fontNumbers: '"Inter Variable", "Inter", sans-serif',
})

/** Every OpenUI mount scopes its tokens to this class instead of the provider's `body` default: the provider writes
 * `--openui-*` here and nothing global, so sibling mounts — a live fence beside a snapshot's forced appearance, the
 * app view behind the panel — never rewrite one another. Every value is a `var()` reference, so the written rule is
 * mode-specific, so concurrent light and dark mounts cannot overwrite each other's unmapped defaults; the provider's unique
 * portal class still carries the tokens to anything OpenUI portals out to `document.body`. */
const SCOPE = 'openui-theuth-scope'
const scopeFor = (mode: 'light' | 'dark') => `${SCOPE}-${mode}`

/** The one theme mount: theuth tokens, scoped to the subtree, `mode` driving only OpenUI's own mode context. */
export function OpenUITheme({ mode, children }: { mode: 'light' | 'dark'; children: ReactNode }) {
  return (
    <ThemeProvider mode={mode} lightTheme={theuthOpenUITheme} cssSelector={`.${scopeFor(mode)}`}>
      <div className={scopeFor(mode)} style={{ display: 'contents' }}>
        {children}
      </div>
    </ThemeProvider>
  )
}
