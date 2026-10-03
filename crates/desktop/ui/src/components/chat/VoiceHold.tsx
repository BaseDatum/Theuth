// 89 VOICE-HOLD: the hold-to-record gesture's floating layer and the composer-row pieces around it. `MicButton`
// runs the reducer (`lib/hold.ts`) and drives everything here. The layer is a portal over the page anchored to the
// mic button; pointer moves write transforms straight to element refs (`track`), React only sees phase changes.

import { ArrowUp, ChevronLeft, ChevronUp, Lock, LockOpen, Mic, Trash2 } from 'lucide-react'
import { type RefObject, useEffect, useImperativeHandle, useRef } from 'react'
import { createPortal } from 'react-dom'
import { Button } from '@/components/ui/button'
import type { FrameSource, VisualFrame } from '@/lib/audio/types'
import { dbToLevel, rmsDb } from '@/lib/audio/decibels'
import { type Hold, LOCK_PX } from '@/lib/hold'
import { cn } from '@/lib/utils'

/** What the composer row shows: `cancelled` is the 420 ms bin wiggle after a slide-left, with the reducer already idle. */
export type Stage = 'idle' | 'held' | 'locked' | 'cancelled'

/** The layer's exit, once the hold ends; the orb and pill animate out before `MicButton` unmounts the layer. */
export type Exit = 'send' | 'cancel' | 'lock' | null

export type Anchor = { x: number; y: number }

/** Durations under `prefers-reduced-motion: reduce` are zero: show and hide are instant. */
export const reduced = () => typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches
export const ms = (n: number) => (reduced() ? 0 : n)

export type LayerHandle = { track: (h: Extract<Hold, { phase: 'held' }>) => void }

/** The orb is 64 px; the lock pill 40 × 88 px with its bottom edge 16 px above the orb. */
const ORB = 64
const PILL_W = 40
const PILL_H = 88
const GAP = 16

/**
 * The floating layer: the orb with its level halo centred on the mic button and the lock pill above it. `source` is
 * the recorder's analyser (the same frames the wave draws); the halo's scale is `1 + 0.7 × level`.
 */
export function VoiceLayer({ anchor, exit, source, ref }: { anchor: Anchor; exit: Exit; source: FrameSource<VisualFrame> | null; ref: RefObject<LayerHandle | null> }) {
  const halo = useRef<HTMLDivElement>(null)
  const pill = useRef<HTMLDivElement>(null)
  const open = useRef<SVGSVGElement>(null)
  const chevron = useRef<SVGSVGElement>(null)

  useImperativeHandle(ref, () => ({
    track(h) {
      const up = Math.max(0, Math.min(1, h.dy / LOCK_PX))
      if (pill.current) pill.current.style.transform = `translateY(${-Math.max(0, h.dy) * 0.45}px)`
      if (chevron.current) chevron.current.style.opacity = String(1 - up)
      if (open.current) open.current.style.color = `color-mix(in oklab, var(--fg-accent) ${Math.round(up * 100)}%, var(--fg-secondary))`
    },
  }))

  // The halo breathes with the mic: fast attack, slow decay, so it never jitters frame to frame. Static under reduced motion.
  useEffect(() => {
    if (!source || exit || reduced()) return
    let level = 0
    return source.subscribe((f) => {
      const target = dbToLevel(f.timeDomain ? rmsDb(f.timeDomain) : f.peakDb)
      level = Math.max(target, level * 0.85)
      if (halo.current) halo.current.style.transform = `scale(${1 + 0.7 * level})`
    })
  }, [source, exit])

  // Leaving: the pill fades (after the lock's bounce), the halo collapses with the orb on cancel and send.
  useEffect(() => {
    if (!exit) return
    const p = pill.current
    if (p) {
      p.style.transition = `opacity ${ms(exit === 'lock' ? 220 : 150)}ms ease-out ${ms(exit === 'lock' ? 180 : 0)}ms`
      p.style.opacity = '0'
    }
    const h = halo.current
    if (h) {
      h.style.transition = `transform ${ms(exit === 'lock' ? 220 : 200)}ms ease-in ${ms(exit === 'lock' ? 180 : 0)}ms, opacity ${ms(200)}ms ease-in`
      h.style.transform = 'scale(0)'
      h.style.opacity = '0'
    }
  }, [exit])

  const half = ORB / 2
  const orbExit =
    exit === 'cancel'
      ? { transform: 'scale(0)', opacity: 0, transition: `transform ${ms(200)}ms ease-in, opacity ${ms(200)}ms ease-in` }
      : exit === 'send'
        ? { transform: 'scale(0.56)', opacity: 0, transition: `transform ${ms(160)}ms ease-in, opacity ${ms(160)}ms ease-in` }
        : exit === 'lock'
          ? { transform: 'scale(0.56)', opacity: 0, transition: `transform ${ms(220)}ms ease-in-out ${ms(180)}ms, opacity ${ms(220)}ms ease-in ${ms(180)}ms` }
          : undefined

  return createPortal(
    <div className="pointer-events-none fixed inset-0 z-70" aria-hidden>
      <div ref={halo} className="absolute size-16 rounded-full bg-danger opacity-[0.22]" style={{ left: anchor.x - half, top: anchor.y - half }} />
      <div
        className={cn('absolute flex size-16 items-center justify-center rounded-full bg-danger text-primary-foreground shadow-lg', !exit && 'voice-orb-in')}
        style={{ left: anchor.x - half, top: anchor.y - half, ...orbExit }}
      >
        <Mic className="size-7" />
      </div>
      <div
        ref={pill}
        className="absolute flex flex-col items-center justify-between rounded-full border border-border-subtle bg-popover py-2.5 shadow-lg motion-safe:transition-[opacity,translate] motion-safe:duration-220 motion-safe:ease-out motion-safe:starting:translate-y-2 motion-safe:starting:opacity-0"
        style={{ left: anchor.x - PILL_W / 2, top: anchor.y - half - GAP - PILL_H, width: PILL_W, height: PILL_H }}
      >
        {exit === 'lock' ? <Lock className="voice-bounce size-5 text-fg-accent" /> : <LockOpen ref={open} className="size-5 text-fg-secondary" />}
        <ChevronUp ref={chevron} className="voice-bob size-4 text-fg-tertiary" />
      </div>
    </div>,
    document.body,
  )
}

/** Left of the wave: the red dot and `m:ss`; after a slide-left the bin takes their place for the 420 ms wiggle. */
export function RecordingMeta({ secs, stage }: { secs: number; stage: Stage }) {
  const clock = `${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, '0')}`
  return (
    <div className="flex h-9 min-w-12 shrink-0 items-center justify-center gap-2 px-1 text-xs tabular-nums text-fg-secondary select-none" aria-live="polite">
      {stage === 'cancelled' ? (
        <Trash2 className="voice-trash size-4 text-danger" aria-label="Recording deleted" />
      ) : (
        <>
          <span className="size-2 rounded-full bg-danger motion-safe:animate-pulse" aria-hidden />
          <span>{clock}</span>
        </>
      )}
    </div>
  )
}

/** "‹ slide to cancel" where the placeholder was; `MicButton` moves and fades it with the finger through `ref`. */
export function CancelHint({ ref }: { ref: RefObject<HTMLSpanElement | null> }) {
  return (
    <span ref={ref} className="pointer-events-none absolute inset-y-0 left-3 flex items-center gap-0.5 text-sm text-fg-tertiary select-none" aria-hidden>
      <ChevronLeft className="size-3.5" />
      <span className="voice-hint">slide to cancel</span>
    </span>
  )
}

/** The locked bar's left end. */
export function DeleteNote({ onClick }: { onClick: () => void }) {
  return (
    <Button variant="ghost" className="size-9 shrink-0 rounded-full text-fg-secondary hover:text-danger" aria-label="Delete recording" onClick={onClick}>
      <Trash2 className="size-5" />
    </Button>
  )
}

/** The locked bar's right end, in the mic button's place. */
export function SendNote({ onClick, disabled, ref }: { onClick: () => void; disabled?: boolean; ref?: RefObject<HTMLButtonElement | null> }) {
  return (
    <Button ref={ref} className="size-9 shrink-0 rounded-full bg-fg-accent text-primary-foreground hover:bg-fg-accent-hover" aria-label="Send voice note" disabled={disabled} onClick={onClick}>
      <ArrowUp className="size-[18px]" />
    </Button>
  )
}
