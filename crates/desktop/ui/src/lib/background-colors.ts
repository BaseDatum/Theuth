/**
 * One palette for background dots, card marks, sidebar glyphs and the dashes on the bar above the composer: every dash
 * style lives here. `dash`: a width other than the bar's standard dash (`ActivityDash`).
 */
export const backgroundColors = {
  todos: { bg: 'bg-project-sand', text: 'text-project-sand' },
  subagents: { bg: 'bg-project-malachite', text: 'text-project-malachite' },
  approval: { bg: 'bg-warning', text: 'text-warning' },
  jobs: { bg: 'bg-project-lapis', text: 'text-project-lapis' },
  terminals: { bg: 'bg-project-faience', text: 'text-project-faience' },
  /** B2: a lapis pill with a small foreground dot at its centre. */
  browser: { bg: 'bg-project-lapis bg-[radial-gradient(circle,var(--foreground)_0_1.6px,transparent_2px)]', text: 'text-project-lapis' },
  /** A1: solid sand, 70 % of the standard dash's width. */
  artifacts: { bg: 'bg-project-sand', text: 'text-project-sand', dash: 'w-[11px]' },
  /** V4: slate with an umber end cap. */
  desktop: { bg: 'bg-[linear-gradient(90deg,var(--project-slate)_0_68%,var(--project-umber)_68%_100%)]', text: 'text-project-slate' },
  /** The tray's Code map tile only: the CodeMap has no dash on the bar. */
  codemap: { bg: 'bg-fg-tertiary', text: 'text-fg-tertiary' },
  reminders: { bg: 'bg-project-plum', text: 'text-project-plum' },
} as const
