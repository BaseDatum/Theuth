// The composer's microphone as one gesture control (76 §0.12; reducer in `lib/hold.ts`). Tap → dictation.
// Hold → a ring fills around the button over `HOLD_MS`; at the threshold a voice note records while the finger is
// down (filled button, pulsing ring, a buzz): a timer, "slide down to cancel", a lock glyph that fills as the finger
// travels up; past the threshold it locks hands-free and the button becomes stop-and-send.
// Pointer events with capture, so the release lands here wherever the finger went.

import { Lock, LockOpen, Mic, Square, X } from 'lucide-react'
import { type PointerEvent as ReactPointerEvent, useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { type Act, CANCEL_PX, HOLD_MS, type Hold, IDLE, LOCK_PX, ring, step, type Verb } from '@/lib/hold'
import { cn } from '@/lib/utils'

/** A short buzz where the platform has one (Android WebView/Chrome); iOS WebKit has no vibration API — silently none. */
const buzz = () => navigator.vibrate?.(15)
/** Circumference of the progress ring (r = 18 in a 40 px box, 2 px outside the 36 px button). */
const RING = 2 * Math.PI * 18

export function MicButton({
  recording,
  secs,
  disabled,
  reason,
  onTap,
  onRecord,
  onStop,
}: {
  recording: boolean
  secs: number
  disabled: boolean
  /** Why dictation cannot start now (the tooltip); the hold still records. */
  reason: string | null
  onTap: () => void
  onRecord: () => void
  onStop: (discard: boolean) => void
}) {
  const [hold, setHold] = useState<Hold>(IDLE)
  const ref = useRef<HTMLButtonElement>(null)
  const timer = useRef(0)
  /** The pointer sequence that locked also ends in a `click`; it must not stop-and-send. */
  const swallow = useRef(false)
  const state = useRef(hold)
  state.current = hold

  const act = (a: Act) => {
    const [next, verb] = step(state.current, a)
    state.current = next
    setHold(next)
    run(verb)
  }
  const run = (verb: Verb) => {
    if (verb === 'tap') onTap()
    else if (verb === 'record') {
      buzz()
      onRecord()
    } else if (verb === 'lock') {
      buzz()
      swallow.current = true
    } else if (verb === 'send') onStop(false)
    else if (verb === 'cancel') onStop(true)
  }

  // The recording ended by any path (send, cancel, Escape, a failed start): the control is a mic again.
  useEffect(() => {
    if (!recording && state.current.phase !== 'pressed') act({ type: 'reset' })
  }, [recording])
  useEffect(() => () => clearTimeout(timer.current), [])

  const down = (e: ReactPointerEvent<HTMLButtonElement>) => {
    if (disabled || e.button !== 0) return
    e.currentTarget.setPointerCapture(e.pointerId)
    clearTimeout(timer.current)
    timer.current = window.setTimeout(() => act({ type: 'timer' }), HOLD_MS)
    act({ type: 'down', x: e.clientX, y: e.clientY })
  }
  const move = (e: ReactPointerEvent<HTMLButtonElement>) => act({ type: 'move', x: e.clientX, y: e.clientY })
  const up = (e: ReactPointerEvent<HTMLButtonElement>) => {
    clearTimeout(timer.current)
    const r = ref.current?.getBoundingClientRect()
    const inside = !!r && e.clientX >= r.left && e.clientX <= r.right && e.clientY >= r.top && e.clientY <= r.bottom
    act({ type: 'up', x: e.clientX, y: e.clientY, inside })
  }

  const held = hold.phase === 'held'
  const locked = hold.phase === 'locked'
  const live = recording && (held || locked)
  const towardsLock = held ? Math.max(0, Math.min(1, hold.dy / LOCK_PX)) : 0
  const towardsCancel = held ? Math.max(0, Math.min(1, -hold.dy / CANCEL_PX)) : 0
  const clock = `${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, '0')}`
  // B5: the ring fills over `HOLD_MS` from the press (a transition, so a release collapses it), snaps full at the
  // threshold and pulses while the note records. ponytail: steady pulse — the recorder exposes no level.
  const fill = ring(hold)

  return (
    <div className="flex shrink-0 items-center gap-1">
      {live && (
        <div className="flex items-center gap-2 text-xs tabular-nums text-fg-secondary select-none" aria-live="polite">
          <span className="size-2 rounded-full bg-danger motion-safe:animate-pulse" aria-hidden />
          <span>{clock}</span>
          {held && (
            <span className="hidden text-fg-tertiary sm:inline" style={{ opacity: 1 - towardsCancel * 0.7 }}>
              slide down to cancel
            </span>
          )}
          {held && (
            <span className="relative size-4 text-fg-tertiary" aria-hidden>
              <LockOpen className="absolute inset-0 size-4" style={{ opacity: 1 - towardsLock }} />
              <Lock className="absolute inset-0 size-4 text-fg-accent" style={{ opacity: towardsLock }} />
            </span>
          )}
        </div>
      )}
      {locked && recording && (
        <Button variant="ghost" className="size-9 shrink-0 rounded-full text-fg-secondary" aria-label="Cancel recording" onClick={() => onStop(true)}>
          <X className="size-5" />
        </Button>
      )}
      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            ref={ref}
            variant={live ? 'destructive' : 'ghost'}
            className={cn(
              'relative size-9 shrink-0 rounded-full select-none touch-none transition-transform',
              held && 'scale-110',
              !live && reason !== null && 'opacity-50',
            )}
            aria-label={locked ? 'Stop and send' : recording ? 'Recording' : 'Microphone'}
            aria-disabled={!live && reason !== null}
            disabled={disabled}
            onPointerDown={locked ? undefined : down}
            onPointerMove={locked ? undefined : move}
            onPointerUp={locked ? undefined : up}
            onPointerCancel={locked ? undefined : up}
            onClick={
              locked
                ? () => {
                    if (swallow.current) swallow.current = false
                    else onStop(false)
                  }
                : undefined
            }
            onContextMenu={(e) => e.preventDefault()}
          >
            <svg
              className={cn('pointer-events-none absolute -inset-0.5', fill === 'full' ? 'text-danger' : 'text-fg-accent', fill === 'full' && recording && 'motion-safe:animate-pulse')}
              viewBox="0 0 40 40"
              aria-hidden
            >
              <circle
                cx="20"
                cy="20"
                r="18"
                fill="none"
                stroke="currentColor"
                strokeWidth="2"
                strokeDasharray={RING}
                strokeDashoffset={fill === 'empty' ? RING : 0}
                transform="rotate(-90 20 20)"
                style={{ transition: fill === 'filling' ? `stroke-dashoffset ${HOLD_MS}ms linear` : fill === 'empty' ? 'stroke-dashoffset 150ms ease-out' : 'none' }}
              />
            </svg>
            {locked ? <Square className="size-4 fill-current" /> : <Mic className="size-5" />}
          </Button>
        </TooltipTrigger>
        <TooltipContent>{locked ? 'Stop and send' : (reason ?? 'Dictation (Ctrl+D) · hold to record a voice note')}</TooltipContent>
      </Tooltip>
    </div>
  )
}
