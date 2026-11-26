import { cn } from '@/lib/utils'

/**
 * The one row marker of the panel above the composer: a vertical bar in the category's colour
 * (`backgroundColors[...].bg`), the same size for every category — card headers, tiles and the rows inside a card.
 */
export const CATEGORY_MARK = 'h-4 w-1 shrink-0 rounded-full'

export function CategoryMark({ color, className }: { color: string; className?: string }) {
  return <span aria-hidden data-category-mark="" className={cn(CATEGORY_MARK, color, className)} />
}
