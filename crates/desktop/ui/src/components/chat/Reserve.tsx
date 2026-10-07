import { type ReactNode, useContext, useEffect, useRef } from 'react'
import { MeasuredContext, recall, remember } from '@/lib/measured'
import { cn } from '@/lib/utils'

/**
 * 135 §2: reserves the last measured height of an async block while it renders again, then records the settled
 * height. `id` is `<entry>/<hash>`; `null` (or no transcript around: artifact viewers, boards) renders a plain div.
 * While reserved the box is a one-cell grid, so a placeholder child fills the remembered height.
 */
export function Reserve({ id, ready, className, children }: { id: string | null; ready: boolean; className?: string; children: ReactNode }) {
  const m = useContext(MeasuredContext)
  const ref = useRef<HTMLDivElement>(null)
  const key = m && m.width && id ? id : null
  const reserved = key && !ready ? recall(m!.session, m!.width, key) : undefined
  useEffect(() => {
    const el = ref.current
    if (!key || !ready || !el || typeof ResizeObserver === 'undefined') return
    const { session, width } = m!
    // Border box (`min-height` is border-box under preflight); layout is clean inside the callback.
    const ro = new ResizeObserver(() => el.offsetHeight && remember(session, width, key, el.offsetHeight))
    ro.observe(el)
    return () => ro.disconnect()
  }, [key, ready, m])
  return (
    <div ref={ref} className={cn(className, reserved && 'grid')} style={reserved ? { minHeight: reserved } : undefined}>
      {children}
    </div>
  )
}
