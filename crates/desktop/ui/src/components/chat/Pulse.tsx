import { cn } from '@/lib/utils'

/** ChatGPT's pulsing dot: something is happening. */
export function Pulse({ className }: { className?: string }) {
  return <span className={cn('inline-block size-2.5 shrink-0 animate-pulse rounded-full bg-foreground', className)} />
}
