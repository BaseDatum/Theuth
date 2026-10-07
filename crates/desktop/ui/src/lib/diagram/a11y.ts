// Design 123 §5.1 — SVG accessibility: `role="img" aria-labelledby` → `<title>` (first child) + `<desc>` whose text
// is the first line of the §4.4 lowering, so the a11y text and the terminal agree; `role="group"` when `onSelect`
// is wired (as `Graph.tsx` does).

export interface SvgA11y {
  /** Attributes for the `<svg>` element. */
  svg: { role: 'img' | 'group'; 'aria-labelledby': string }
  /** `id` of the `<title>`; render it as the svg's first child. */
  titleId: string
  /** `id` of the `<desc>`; render it right after the title. */
  descId: string
}

/** Attributes and element ids for one svg; `base` must be unique per mounted svg (a hash or the frame name). */
export function svgA11y(base: string, interactive: boolean): SvgA11y {
  const titleId = `${base}-title`
  const descId = `${base}-desc`
  return { svg: { role: interactive ? 'group' : 'img', 'aria-labelledby': `${titleId} ${descId}` }, titleId, descId }
}

/** `6 tasks` / `1 task` — the one-line summary the stubs, captions and `<desc>` share. */
export function countOf(n: number, unit: string): string {
  return `${n} ${n === 1 ? unit.replace(/ies$/, 'y').replace(/s$/, '') : unit}`
}

/** The `<desc>` text: `Name: 6 tasks`, or `Name: <title> — 6 tasks` when the component has a title. */
export function describe(name: string, title: string | null | undefined, n: number, unit: string): string {
  const count = countOf(n, unit)
  return title ? `${name}: ${title} — ${count}` : `${name}: ${count}`
}

/** `aria-label` of one unit: its label, then `sub`/detail and the state when they add information. */
export function unitLabel(label: string, detail?: string | null, state?: string | null): string {
  const parts = [label]
  if (detail) parts.push(detail)
  if (state && state !== 'default') parts.push(state)
  return parts.join(', ')
}
