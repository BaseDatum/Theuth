// Design 71 §2.5: a passage of real code pinned to its location. Mono header `path:start-end · symbol · lang` with a
// copy button, a numbered gutter from `start`, marked rows tinted with a 2 px accent bar, 18 visible rows then
// "Show all", the caption under. Every row carries `data-line` so the board reader (step 4) can register each as a
// `path#L<n>` anchor without touching this file; the live chip (step 7) slots into the header after the meta.
import { code } from '@streamdown/code'
import type { TokensResult } from 'shiki'
import { Copy } from 'lucide-react'
import { type ReactNode, useEffect, useState } from 'react'
import { toast } from 'sonner'
import type { z } from 'zod'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import type { excerptProps } from './catalog'
import { excerptLines } from './parse'

/** R §B.4 `heightMode: capped`: rows shown before the body scrolls. */
export const VISIBLE_ROWS = 18
const ROW_PX = 20

export type ExcerptProps = z.infer<typeof excerptProps>

/**
 * `active`: the rows the board's active anchor covers (`data-anchor-active`, tinted like marks; 71 §2.3 — the
 * reader computes the intersection, this only paints it). `full`: R §B.7 `heightMode: content`, the side pane's
 * uncapped form. `chip`: the board's live chip (71 §2.5, step 7b), after the meta. All absent in chat.
 */
export function Excerpt({ path, text, start = 1, lang, caption, mark, symbol, active, full, chip }: ExcerptProps & { active?: [number, number] | null; full?: boolean; chip?: ReactNode }) {
  const lines = excerptLines(text)
  const end = start + lines.length - 1
  const [tokens, setTokens] = useState<TokensResult | null>(null)
  const [all, setAll] = useState(false)
  useEffect(() => {
    // Highlight after mount (shiki loads its grammar async); the plain rows are already on screen. Unknown `lang`
    // falls back to plain text inside the plugin.
    if (!lang) return
    let live = true
    const src = lines.join('\n')
    const now = code.highlight({ code: src, language: lang as never, themes: code.getThemes() }, (r) => live && setTokens(r))
    if (now) setTokens(now)
    return () => {
      live = false
    }
  }, [lang, text])
  const marked = (n: number) => (mark ?? []).some(([a, b]) => a! <= n && n <= b!)
  const capped = !full && !all && lines.length > VISIBLE_ROWS
  return (
    <div className="flex flex-col overflow-hidden rounded-md border text-xs" data-streamdown="excerpt">
      <div className="flex items-center gap-2 border-b bg-layer-2 py-1 pr-1 pl-3 font-mono text-fg-tertiary">
        <span className="min-w-0 flex-1 truncate">
          <span className="text-foreground">
            {path}:{start}
            {end > start && `-${end}`}
          </span>
          {symbol && <> · {symbol}</>}
          {lang && <> · {lang}</>}
        </span>
        {chip}
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="Copy excerpt"
          onClick={() => navigator.clipboard.writeText(text).then(() => toast.message('Copied'), () => toast.error('Copy failed'))}
        >
          <Copy />
        </Button>
      </div>
      <div className="overflow-auto font-mono leading-5" style={capped ? { maxHeight: VISIBLE_ROWS * ROW_PX } : undefined}>
        <div className="min-w-max py-0.5">
          {lines.map((line, i) => {
            const n = start + i
            const toks = tokens?.tokens[i]
            const lit = active !== null && active !== undefined && active[0] <= n && n <= active[1]
            return (
              <div
                key={n}
                data-line={n}
                data-anchor-active={lit || undefined}
                className={cn('grid min-h-5 grid-cols-[42px_14px_1fr] pr-3', (marked(n) || lit) && 'bg-fg-accent/10 shadow-[inset_2px_0_0_0_var(--color-fg-accent)]')}
              >
                <span className="text-right text-fg-tertiary tabular-nums select-none" aria-hidden>
                  {n}
                </span>
                <span />
                <span className="whitespace-pre">
                  {toks
                    ? toks.map((t, j) => (
                        <span
                          key={j}
                          className="text-(--c) dark:text-[var(--shiki-dark,var(--c))]"
                          style={{ '--c': t.htmlStyle?.color, '--shiki-dark': t.htmlStyle?.['--shiki-dark'] } as React.CSSProperties}
                        >
                          {t.content}
                        </span>
                      ))
                    : line}
                </span>
              </div>
            )
          })}
        </div>
      </div>
      {capped && (
        <button type="button" className="border-t bg-layer-2 py-1 font-mono text-fg-secondary hover:text-foreground" onClick={() => setAll(true)}>
          Show all {lines.length} lines
        </button>
      )}
      {caption && <div className="border-t px-3 py-1.5 font-mono text-fg-tertiary">{caption}</div>}
    </div>
  )
}
