import type { AttentionItem } from '@/rpc/bindings/method.roster.output'

/**
 * 207 D3: "Why here" — the engine's score and every reason with its points, above the item's screen. Nothing for an
 * engine before 207 (no reasons). The texts are the engine's snapshot at derive time; the card's line ticks live.
 */
export function Why({ item }: { item: AttentionItem }) {
  const reasons = item.reasons ?? []
  if (reasons.length === 0) return null
  return (
    <section aria-label="Why here" className="shrink-0 border-b border-border-subtle px-4 py-2 md:px-6">
      <div className="mx-auto flex w-full max-w-[720px] flex-wrap items-baseline gap-x-3 gap-y-1 text-xs text-fg-tertiary">
        <span className="font-semibold tracking-wide uppercase">Why here</span>
        <span className="font-medium text-fg-secondary tabular-nums">{item.score}</span>
        {reasons.map((r) => (
          <span key={r.signal} className="tabular-nums">
            {r.text} <span className="text-fg-secondary">+{r.points}</span>
          </span>
        ))}
      </div>
    </section>
  )
}
