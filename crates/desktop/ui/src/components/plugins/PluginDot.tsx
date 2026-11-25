// 216 §9.1, contract §12.3: a placement's status dot. A span with colour classes (no `●` code point). Frozen after B0.
import { cn } from '@/lib/utils'
import type { Dot } from '@/rpc/bindings/method.plugins.output'

const DOT_CLASS: Record<Exclude<Dot, 'none'>, string> = {
  ok: 'bg-success',
  busy: 'bg-success animate-pulse',
  warn: 'bg-warning',
  error: 'bg-danger',
  off: 'bg-fg-tertiary/40',
}

/** The dot of `dot`; `none` draws nothing. */
export function PluginDot({ dot, className }: { dot: Dot; className?: string }) {
  if (dot === 'none') return null
  return <span aria-hidden data-dot={dot} className={cn('inline-block size-1.5 shrink-0 rounded-full', DOT_CLASS[dot], className)} />
}
