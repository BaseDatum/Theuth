// 76 §0.12 subtitles: the agent's playing sentence (foreground) and the user's live partial (muted). Same font,
// no names, no bubbles; each line ≤ 2 rows, clamped from the *start* so the newest words stay; ≤ 150 ms fades.

import { useLayoutEffect, useRef } from 'react'
import { cn } from '@/lib/utils'

/**
 * Two rows of `text`, the tail kept: when it overflows, leading words go and a `…` leads. Word count is found by
 * bisection on the live element (≤ log₂ n relayouts per change; partials arrive every ~560 ms). The element owns
 * its text node (no React children): an empty `text` leaves the last words in place while the line fades out.
 */
function Line({ text, className }: { text: string; className: string }) {
  const ref = useRef<HTMLParagraphElement>(null)
  useLayoutEffect(() => {
    const el = ref.current
    if (!el || !text) return
    const words = text.split(' ')
    const fits = (k: number) => {
      el.textContent = k === 0 ? text : `…${words.slice(k).join(' ')}`
      return el.scrollHeight <= el.clientHeight + 1
    }
    if (fits(0)) return
    let lo = 1
    let hi = words.length - 1
    while (lo < hi) {
      const mid = (lo + hi) >> 1
      if (fits(mid)) hi = mid
      else lo = mid + 1
    }
    fits(lo)
  }, [text])
  return (
    <p
      ref={ref}
      aria-live="polite"
      className={cn(
        'h-[2lh] w-full overflow-hidden text-center text-lg leading-snug transition-opacity duration-150 motion-reduce:transition-none',
        text ? 'opacity-100' : 'opacity-0',
        className,
      )}
    />
  )
}

export function Subtitles({ agent, user, caption }: { agent: string; user: string; caption: string | null }) {
  return (
    <div className="flex w-full max-w-[42rem] flex-col items-center gap-1 px-6">
      <Line text={agent} className="" />
      <Line text={user} className="text-fg-secondary" />
      <p
        className={cn(
          'mt-3 h-[1lh] text-center text-xs text-fg-tertiary transition-opacity duration-150 motion-reduce:transition-none',
          caption ? 'opacity-100' : 'opacity-0',
        )}
      >
        {caption}
      </p>
    </div>
  )
}
