import { ChevronRight } from 'lucide-react'
import type { AttentionItem } from '@/rpc/bindings/method.roster.output'

/**
 * 207 D3: "Why here" — the engine's score and every reason with its points, above the item's screen. A strip from `md`
 * up (reasons wrap whole, never mid-reason); under `md` (one pane, little height) a closed disclosure. Nothing for an
 * engine before 207 (no reasons). The texts are the engine's snapshot at derive time; the card's line ticks live.
 */
export function Why({ item }: { item: AttentionItem }) {
  const reasons = item.reasons ?? []
  if (reasons.length === 0) return null
  const list = reasons.map((r) => (
    <span key={r.signal} className="whitespace-nowrap tabular-nums">
      {r.text} <span className="text-fg-secondary">+{r.points}</span>
    </span>
  ))
  return (
    <>
      <details className="group shrink-0 border-b border-border-subtle px-4 py-2 text-xs text-fg-tertiary md:hidden">
        <summary className="flex cursor-default list-none items-center gap-2 [&::-webkit-details-marker]:hidden">
          <ChevronRight className="size-3.5 motion-safe:transition-transform group-open:rotate-90" aria-hidden />
          <span className="font-semibold tracking-wide uppercase">Why here</span>
          <span className="font-medium text-fg-secondary tabular-nums">{item.score}</span>
        </summary>
        <div className="mt-1.5 flex flex-wrap gap-x-3 gap-y-1 pl-5.5">{list}</div>
      </details>
      <section aria-label="Why here" className="hidden shrink-0 border-b border-border-subtle px-6 py-2 md:block">
        <div className="mx-auto flex w-full max-w-[720px] flex-wrap items-baseline gap-x-3 gap-y-1 text-xs text-fg-tertiary">
          <span className="font-semibold tracking-wide uppercase">Why here</span>
          <span className="font-medium text-fg-secondary tabular-nums">{item.score}</span>
          {list}
        </div>
      </section>
    </>
  )
}
