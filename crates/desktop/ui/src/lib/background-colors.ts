/**
 * One palette for background dots, card marks, sidebar glyphs and the dashes on the bar above the composer: every dash
 * style lives here. `dash`: a width other than the bar's standard dash (`ActivityDash`).
 */
export const backgroundColors = {
  todos: { bg: 'bg-project-sand', text: 'text-project-sand' },
  subagents: { bg: 'bg-project-malachite', text: 'text-project-malachite' },
  approval: { bg: 'bg-warning', text: 'text-warning' },
  jobs: { bg: 'bg-project-lapis', text: 'text-project-lapis' },
  /** Half light, half dark, outlined in the foreground colour, so it reads on both themes. */
  terminals: { bg: 'box-border border border-foreground bg-[linear-gradient(90deg,#f4ecdf_0_50%,#0c0a09_50%_100%)]', text: 'text-project-faience' },
  /** K1: faience in three short segments with gaps, a row of cells. */
  kernels: { bg: 'bg-[linear-gradient(90deg,var(--project-faience)_0_30%,transparent_30%_35%,var(--project-faience)_35%_65%,transparent_65%_70%,var(--project-faience)_70%_100%)]', text: 'text-project-faience' },
  /** B2: a lapis pill with a small foreground dot at its centre. */
  browser: { bg: 'bg-project-lapis bg-[radial-gradient(circle,var(--foreground)_0_1.6px,transparent_2px)]', text: 'text-project-lapis' },
  /** A1: solid sand, 70 % of the standard dash's width. */
  artifacts: { bg: 'bg-project-sand', text: 'text-project-sand', dash: 'w-[11px]' },
  /** W1: nile with a fine dot grid in the page colour (the canvas's dots), so light and dark both follow the theme. */
  whiteboard: { bg: 'bg-project-nile bg-[radial-gradient(circle,var(--background)_0_0.8px,transparent_1.1px)] bg-size-[4px_5px]', text: 'text-project-nile' },
  /** V4: slate with an umber end cap. */
  desktop: { bg: 'bg-[linear-gradient(90deg,var(--project-slate)_0_68%,var(--project-umber)_68%_100%)]', text: 'text-project-slate' },
  /** The tray's Code map tile only: the CodeMap has no dash on the bar. */
  codemap: { bg: 'bg-fg-tertiary', text: 'text-fg-tertiary' },
  reminders: { bg: 'bg-project-plum', text: 'text-project-plum' },
} as const
