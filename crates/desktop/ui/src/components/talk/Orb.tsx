// 76 §0.12: the orb *is* the state. `@shadercn/orb-16` (WebGPU, its own state palette — no theme tint) when the
// webview has it and motion is welcome;
// otherwise a CSS orb that shows the same three states without a frame loop (reduced motion: no animation).

import { useState } from 'react'
import { Orb16 } from '@/components/orbs/orb-16'
import type { OrbState } from '@/lib/talk'
import { cn } from '@/lib/utils'

const reduced = () => matchMedia('(prefers-reduced-motion: reduce)').matches

export function Orb({ state, size, onClick, label }: { state: OrbState; size: number; onClick: () => void; label: string }) {
  const [gpu] = useState(() => 'gpu' in navigator && !reduced())
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      className="group relative shrink-0 rounded-full outline-none focus-visible:ring-2 focus-visible:ring-fg-accent/60"
      style={{ width: size, height: size }}
    >
      {gpu ? (
        <Orb16 size={size} state={state} ariaLabel="" className="pointer-events-none" />
      ) : (
        <span
          aria-hidden
          data-state={state}
          className={cn(
            'talk-orb block size-full rounded-full',
            state === 'thinking' && 'talk-orb-thinking',
            state === 'speaking' && 'talk-orb-speaking',
          )}
        />
      )}
    </button>
  )
}
