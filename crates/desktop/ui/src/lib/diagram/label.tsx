// Design 123 §5.1, §5.4 — an edge/flow label on an opaque `card` mask 6–10 px off its stroke, never on the line;
// ≤ 14 characters shown (the full text goes to the tooltip / `<title>`).
import type { DiagramTokens } from '../diagram-theme'
import type { Point } from './types'

export const LABEL_PX = 12
export const LABEL_MAX_CHARS = 14
/** Gap between a label's mask and the stroke it annotates. */
export const LABEL_GAP = 8
const PAD_X = 4
const PAD_Y = 2
const CHAR_W = 6.6

/** `text` cut to `max` characters with an ellipsis; unchanged when it fits. */
export function truncate(text: string, max = LABEL_MAX_CHARS): string {
  const t = text.replace(/\s+/g, ' ').trim()
  return t.length <= max ? t : `${t.slice(0, Math.max(1, max - 1)).trimEnd()}…`
}

/** Mask size for the shown text. */
export function labelBox(shown: string): { w: number; h: number } {
  return { w: Math.ceil(shown.length * CHAR_W + 2 * PAD_X), h: LABEL_PX + 2 * PAD_Y }
}

/**
 * The label centred at `at`, offset `LABEL_GAP` perpendicular to a `horizontal`/`vertical` stroke (`above`/`beside`
 * pick the side), on a `card`-filled mask. `full` (defaults to `text`) is the `<title>` for the hover.
 */
export function MaskedLabel({ text, full, at, along, side = 'before', tokens: t, fill }: { text: string; full?: string; at: Point; along: 'horizontal' | 'vertical'; side?: 'before' | 'after'; tokens: DiagramTokens; fill?: string }) {
  const shown = truncate(text)
  if (!shown) return null
  const box = labelBox(shown)
  const k = side === 'before' ? -1 : 1
  const cx = along === 'horizontal' ? at.x : at.x + k * (LABEL_GAP + box.w / 2)
  const cy = along === 'horizontal' ? at.y + k * (LABEL_GAP + box.h / 2) : at.y
  return (
    <g>
      <title>{full ?? text}</title>
      <rect x={cx - box.w / 2} y={cy - box.h / 2} width={box.w} height={box.h} rx={3} fill={t.card} />
      <text x={cx} y={cy} dy="0.32em" textAnchor="middle" fontFamily={t.font} fontSize={LABEL_PX} fill={fill ?? t.mutedForeground}>
        {shown}
      </text>
    </g>
  )
}
