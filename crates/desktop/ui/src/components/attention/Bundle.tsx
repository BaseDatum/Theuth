import { Badge } from '@/components/ui/badge'
import { Input } from '@/components/ui/input'
import { cn } from '@/lib/utils'
import type { Item } from '@/rpc/bindings/Item'

/**
 * 103 F3: the N sub-decisions of one escalation as a numbered list — title (a button that scrolls the embedded board to
 * the `###` heading whose slug is the item id, when `anchored`), description, option chips with the recommended one
 * pre-picked, "Type your own…" per item (typing clears the chip; picking clears the text). The Approve all / Answer
 * row is the screen's (`DecisionScreen`), so the top-level options and the bundle share one submit.
 */
export function Bundle({ items, picks, onPick, anchored }: { items: Item[]; picks: Pick[]; onPick: (i: number, pick: Pick) => void; anchored: boolean }) {
  return (
    <ol className="flex flex-col gap-4">
      {items.map((it, i) => {
        const pick = picks[i] ?? null
        const text = typeof pick === 'string' ? pick : ''
        return (
          <li key={it.id} className="flex gap-2 text-sm">
            <span className="w-5 shrink-0 pt-px text-right tabular-nums text-fg-tertiary">{i + 1}.</span>
            <div className="flex min-w-0 flex-1 flex-col gap-1.5">
              {anchored ? (
                <button type="button" className="w-fit text-left font-medium hover:underline" title="Show in the board" onClick={() => jumpTo(it)}>
                  {it.title}
                </button>
              ) : (
                <span className="font-medium">{it.title}</span>
              )}
              {it.description && <span className="text-xs text-fg-secondary">{it.description}</span>}
              {it.options.length > 0 && (
                <div className="flex flex-wrap gap-1.5" role="group" aria-label={`Options for ${it.title}`}>
                  {it.options.map((o, j) => {
                    const on = pick === j
                    return (
                      <button
                        key={o.label}
                        type="button"
                        aria-pressed={on}
                        title={o.description ?? undefined}
                        onClick={() => onPick(i, j)}
                        className={cn('flex min-h-7 items-center gap-1 rounded-full border px-2.5 py-0.5 text-xs transition-colors', on ? 'border-fg-accent bg-accent' : 'border-border-subtle text-fg-secondary hover:bg-accent/60')}
                      >
                        {o.label}
                        {it.recommended === j && (
                          <Badge variant="secondary" className="px-1 py-0 text-[10px]">
                            recommended
                          </Badge>
                        )}
                      </button>
                    )
                  })}
                </div>
              )}
              <Input
                value={text}
                onChange={(e) => onPick(i, e.target.value === '' ? null : e.target.value)}
                placeholder="Type your own…"
                aria-label={`Your own answer for ${it.title}`}
                className={cn('h-8 text-base md:text-sm', text.trim() && 'border-fg-accent')}
              />
            </div>
          </li>
        )
      })}
    </ol>
  )
}

/**
 * The board reader gives headings their block id, not a slug, so the match is on text: the `###` whose slug is the item
 * id, else the one whose slug is the title's. A collapsed section has no mounted heading — then nothing happens.
 */
function jumpTo(it: Item) {
  const hs = [...document.querySelectorAll<HTMLElement>('.packet-board h3')]
  const h = hs.find((h) => slug(h.textContent ?? '') === it.id) ?? hs.find((h) => slug(h.textContent ?? '') === slug(it.title))
  h?.scrollIntoView({ block: 'start', behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth' })
}


/** One item's draft: an option index, the typed text, or nothing yet. */
export type Pick = number | string | null

/** The label an item's pick resolves to (`''` when there is none). */
export function pickLabel(item: Item, pick: Pick): string {
  if (typeof pick === 'string') return pick.trim()
  if (pick === null) return ''
  return item.options[pick]?.label ?? ''
}

/** Every item has an option or typed text. */
export function allAnswered(items: Item[], picks: Pick[]): boolean {
  return items.every((it, i) => pickLabel(it, picks[i] ?? null) !== '')
}

/** Every item has a recommendation and still sits on it (Approve all as recommended). */
export function allRecommended(items: Item[], picks: Pick[]): boolean {
  return items.length > 0 && items.every((it, i) => it.recommended !== null && picks[i] === it.recommended)
}

/**
 * The structured answer the thread and the caller read without parsing: `1. <title>: <pick>` per item, then the
 * top-level pick or text (when any), joined with newlines.
 */
export function bundleAnswer(items: Item[], picks: Pick[], top = ''): string {
  const lines = items.map((it, i) => `${i + 1}. ${it.title}: ${pickLabel(it, picks[i] ?? null)}`)
  if (top.trim()) lines.push(top.trim())
  return lines.join('\n')
}

/** The first line of a markdown text, ≤ 140 chars (`tasks::packet::head`). */
export function head(text: string): string {
  const line = text.split('\n').find((l) => l.trim() !== '')?.trim() ?? ''
  return line.length > 140 ? `${line.slice(0, 139)}…` : line
}

/** A `###` heading's text as the slug an `Item.id` is matched against (lower-case, `-` between runs of [a-z0-9]). */
export function slug(text: string): string {
  return text
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
}
