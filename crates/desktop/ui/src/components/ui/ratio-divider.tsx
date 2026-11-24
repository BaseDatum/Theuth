import { useRef } from 'react'
import { ResizeHandle } from '@/components/ui/resize-handle'
import { clampRatio, RATIO_MAX, RATIO_MIN, RATIO_STEP } from '@/lib/resize-drag'

/**
 * Between two flex children sized `flex: r 1 0%` / `flex: 1-r 1 0%`; a drag reports the new first-child share
 * (0.15–0.85) to `onRatio`. With `ratio` the separator also exposes its value and nudges it by 5 % per arrow key.
 */
export function Divider({ dir, ratio, onRatio }: { dir: 'row' | 'col'; ratio?: number; onRatio: (ratio: number) => void }) {
  const rect = useRef<DOMRect>(null)
  return (
    <ResizeHandle
      orientation={dir === 'row' ? 'vertical' : 'horizontal'}
      value={ratio === undefined ? undefined : { now: Math.round(ratio * 100), min: RATIO_MIN * 100, max: RATIO_MAX * 100 }}
      onStart={(_, el) => (rect.current = el.parentElement?.getBoundingClientRect() ?? null)}
      onMove={(x, y) => {
        const r = rect.current
        if (r) onRatio(clampRatio(dir === 'row' ? (x - r.left) / r.width : (y - r.top) / r.height))
      }}
      onNudge={ratio === undefined ? undefined : (d) => onRatio(clampRatio(ratio + d * RATIO_STEP))}
    />
  )
}
