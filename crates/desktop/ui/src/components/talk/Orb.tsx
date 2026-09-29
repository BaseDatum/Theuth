// 76 §0.12: the orb *is* the state. `@shadercn/orb-20` (WebGPU) when the webview has it and motion is welcome;
// otherwise a CSS orb that shows the same three states without a frame loop (reduced motion: no animation).

import { useState } from 'react'
import { Orb20 } from '@/components/orbs/orb-20'
import type { OrbState } from '@/lib/talk'
import { cn } from '@/lib/utils'

const reduced = () => matchMedia('(prefers-reduced-motion: reduce)').matches

/** The theme's accent, read once per mount: the shader takes a hex, not a CSS variable. */
function accent(): string {
  const v = getComputedStyle(document.documentElement).getPropertyValue('--fg-accent').trim()
  return /^#[0-9a-f]{6}$/i.test(v) ? v : '#b4531e'
}

export function Orb({ state, size, onClick, label }: { state: OrbState; size: number; onClick: () => void; label: string }) {
  const [gpu] = useState(() => 'gpu' in navigator && !reduced())
  const [tint] = useState(accent)
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
        <Orb20 size={size} state={state} colors={{ tint }} ariaLabel="" className="pointer-events-none" />
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
