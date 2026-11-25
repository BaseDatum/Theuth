// The composer bar's model label, shortened in steps as the bar narrows (option A of the collapse mock): the full
// `Claude Opus 5.5 · medium`, then `Opus 5.5` (no effort, no vendor word), then `Opus`, then a sparkle icon alone.
// Each step is the same control with a shorter label; the step is the longest that fits the room the buttons leave.

/** The label at each text step: `full` (name · effort), `short` (no vendor word, no effort), `family` (one word). */
export type ModelLabels = { full: string; short: string; family: string }

/** 0 full · 1 short · 2 family · 3 the icon alone. */
export type ModelStep = 0 | 1 | 2 | 3

/** A bare model id rather than a display name: lowercase, no spaces (`claude-opus-5-5`, `openai/gpt-6-mini`). */
const isId = (name: string): boolean => /^[a-z0-9._:/-]+$/.test(name)

/** A word that is only letters, and so may be a vendor (`Claude`), never a versioned family (`GPT-6`). */
const plain = (w: string): boolean => /^[A-Za-z]+$/.test(w)

/**
 * Pure: the three text steps of `name` (the model's display name, else its id) and `effort`. A leading vendor word
 * goes at the second step when another all-letter word follows it (`Claude Opus 5.5` → `Opus 5.5`; `GPT-6 mini`,
 * `Gemini 3 Pro` and `deepseek-v4.1-flash` keep theirs). The family is the first word left, cut before a hyphenated version
 * (`GPT-6` → `GPT`, `DeepSeek-V4.1-Flash` → `DeepSeek`). An id splits on its hyphens after any `provider/` prefix.
 */
export function modelLabels(name: string, effort?: string | null): ModelLabels {
  const full = effort ? `${name} · ${effort}` : name
  const id = isId(name)
  const sep = id ? '-' : ' '
  let words = (id ? (name.split('/').pop() ?? name) : name).split(sep).filter(Boolean)
  if (words.length > 1 && plain(words[0]!) && plain(words[1]!)) words = words.slice(1)
  const short = words.join(sep) || name
  const head = words[0] ?? name
  const family = id ? head : (head.replace(/-[vV]?\d.*$/, '') || head)
  return { full, short, family }
}

/**
 * Pure: the longest step whose label fits `room` px (the bar's slot the buttons leave); the icon when none does.
 * `widths` are the rendered widths of the full, short and family triggers.
 */
export function modelStep(room: number, widths: readonly [number, number, number]): ModelStep {
  const i = widths.findIndex((w) => w <= room)
  return (i < 0 ? 3 : i) as ModelStep
}

/** The trigger's chrome around its text: 8 px padding each side, the 4 px gap and the 14 px chevron. */
export const LABEL_CHROME = 34

let ctx: CanvasRenderingContext2D | null | undefined
/** The width of `text` in the label's 13 px type, by canvas; about 7 px a character where there is no canvas (tests). */
export function textWidth(text: string, font = '13px system-ui, sans-serif'): number {
  if (ctx === undefined) {
    try {
      ctx = typeof document === 'undefined' ? null : document.createElement('canvas').getContext('2d')
    } catch {
      ctx = null
    }
  }
  if (!ctx) return text.length * 7
  ctx.font = font
  return Math.ceil(ctx.measureText(text).width)
}
