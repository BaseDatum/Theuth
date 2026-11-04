// Vendored from nteract nteract/nteract:src/components/cell/gutter-colors.ts (adapted for theuth 171 §16)
// 171 §11.8: the fixed Tailwind palette (sky/emerald/amber/purple/rose/gray) is replaced once with theuth tokens.
export interface GutterColorConfig {
  ribbon: {
    default: string;
    focused: string;
  };
  outputRibbon: {
    default: string;
    focused: string;
  };
  background: {
    focused: string;
  };
}

/** One config per accent token; every cell type shares the neutral default ribbon. */
function tokenGutter(ribbon: string, outputRibbon: string, background: string): GutterColorConfig {
  return {
    ribbon: { default: "bg-border", focused: ribbon },
    outputRibbon: {
      default: "bg-gradient-to-b from-border/40 to-border",
      focused: outputRibbon,
    },
    background: { focused: background },
  };
}

/**
 * Default ribbon colors for built-in cell types.
 * Colors are designed to keep cell-type ribbons visually consistent.
 */
export const defaultGutterColors: Record<string, GutterColorConfig> = {
  code: tokenGutter("bg-info", "bg-gradient-to-b from-info/40 to-info", "bg-info-bg/40"),
  markdown: tokenGutter("bg-success", "bg-gradient-to-b from-success/40 to-success", "bg-success-bg/40"),
  sql: tokenGutter("bg-warning", "bg-gradient-to-b from-warning/40 to-warning", "bg-warning-bg/40"),
  ai: tokenGutter("bg-fg-accent", "bg-gradient-to-b from-fg-accent/40 to-fg-accent", "bg-layer-2"),
  raw: tokenGutter("bg-danger", "bg-gradient-to-b from-danger/40 to-danger", "bg-danger-bg/40"),
};

/**
 * Fallback colors for unknown cell types.
 * Uses neutral styling.
 */
export const fallbackGutterColors: GutterColorConfig = tokenGutter(
  "bg-border-strong",
  "bg-gradient-to-b from-border-strong/40 to-border-strong",
  "bg-layer-2",
);

/**
 * Get ribbon colors for a cell type.
 * Falls back to neutral for unknown types.
 *
 * @param cellType - The cell type to get colors for
 * @param customColors - Optional custom color overrides
 */
export function getGutterColors(
  cellType: string,
  customColors?: Record<string, GutterColorConfig>,
): GutterColorConfig {
  return customColors?.[cellType] ?? defaultGutterColors[cellType] ?? fallbackGutterColors;
}
