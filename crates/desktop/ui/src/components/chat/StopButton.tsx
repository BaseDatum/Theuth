// 133 STOP-SCOPE §4: the composer's stop button. Tap → stop the root turn. Press-and-hold (89's reducer, `lib/hold.ts`,
// same numbers as the mic) → a pill floats above the button: "↑ Stop everything · N agents"; slide up `LOCK_PX` arms
// it (the pill fills, a haptic); release armed → stop the whole tree, release unarmed → the root only. `ring` (0..1)
// is the Esc hold from `lib/stop-gesture.ts`: it fills a ring around the button under the label "Stop everything".

import { Square } from 'lucide-react'
import { type PointerEvent as ReactPointerEvent, useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { Button } from '@/components/ui/button'
import { haptics } from '@/lib/haptics'
import { type Act, HOLD_MS, type Hold, IDLE, step } from '@/lib/hold'
import { cn } from '@/lib/utils'
import type { CancelScope } from '@/rpc/bindings/method.agent.input'

const RING = 44
const STROKE = 2.5
const CIRC = Math.PI * (RING - STROKE)

export function StopButton({ count, ring, onStop }: { count: number; ring: number; onStop: (scope: CancelScope) => void }) {
  const [hold, setHold] = useState<Hold>(IDLE)
  const [anchor, setAnchor] = useState<{ x: number; y: number } | null>(null)
  const ref = useRef<HTMLButtonElement>(null)
  const timer = useRef(0)
  const state = useRef(hold)
  state.current = hold

  const act = (a: Act) => {
    const [next, verb] = step(state.current, a)
    state.current = next
    setHold(next)
    if (verb === 'tap' || verb === 'send') onStop('root')
    else if (verb === 'record') {
      haptics.tick()
      const r = ref.current?.getBoundingClientRect()
      if (r) setAnchor({ x: r.left + r.width / 2, y: r.top })
    } else if (verb === 'lock') haptics.lock()
    // `cancel` (a slide left) just ends the gesture: no horizontal affordance here.
  }
  const reset = () => {
    clearTimeout(timer.current)
    act({ type: 'reset' })
    setAnchor(null)
  }

  const held = hold.phase === 'held'
  const armed = hold.phase === 'locked'
  useEffect(() => {
    if (!held && !armed) return
    document.body.classList.add('voice-hold')
    window.addEventListener('blur', reset)
    return () => {
      document.body.classList.remove('voice-hold')
      window.removeEventListener('blur', reset)
    }
  }, [held || armed])
  useEffect(() => () => clearTimeout(timer.current), [])

  const down = (e: ReactPointerEvent<HTMLButtonElement>) => {
    if (e.button !== 0) return
    e.currentTarget.setPointerCapture(e.pointerId)
    clearTimeout(timer.current)
    timer.current = window.setTimeout(() => act({ type: 'timer' }), HOLD_MS)
    act({ type: 'down', x: e.clientX, y: e.clientY })
  }
  const move = (e: ReactPointerEvent<HTMLButtonElement>) => act({ type: 'move', x: e.clientX, y: e.clientY })
  const up = (e: ReactPointerEvent<HTMLButtonElement>) => {
    clearTimeout(timer.current)
    if (state.current.phase === 'locked') {
      onStop('tree')
      reset()
    } else {
      act({ type: 'up', x: e.clientX, y: e.clientY })
      setAnchor(null)
    }
  }

  const dy = held ? Math.max(0, hold.dy) : 0
  const pill = anchor && (held || armed)
  const label = `${pill ? '\u2191 ' : ''}Stop everything${pill && count > 0 ? ` \u00b7 ${count} agent${count === 1 ? '' : 's'}` : ''}`
  const showLabel = pill || ring > 0
  return (
    <>
      {showLabel &&
        createPortal(
          <div className="pointer-events-none fixed inset-0 z-70" aria-hidden>
            <div
              className={cn(
                'absolute -translate-x-1/2 whitespace-nowrap rounded-full border px-3 py-1.5 text-xs font-medium shadow-lg motion-safe:transition-colors motion-safe:duration-120',
                armed ? 'border-danger bg-danger text-primary-foreground' : 'border-border-subtle bg-popover text-fg-secondary',
              )}
              style={{
                left: (anchor ?? at(ref.current)).x,
                top: (anchor ?? at(ref.current)).y - 12 - 30 - dy * 0.45,
              }}
            >
              {label}
            </div>
          </div>,
          document.body,
        )}
      <Button
        ref={ref}
        // `data-drag-row` (styles.css): no text selection or iOS callout under a held finger.
        data-drag-row
        className={cn(
          'relative size-9 shrink-0 rounded-full bg-danger text-primary-foreground select-none touch-none hover:bg-danger/90 duration-120 ease-out motion-reduce:transition-none',
          hold.phase === 'pressed' && 'scale-[1.12]',
          armed && 'scale-[1.2]',
        )}
        aria-label={armed ? 'Release to stop everything' : held ? 'Stop - slide up to stop everything' : 'Stop'}
        onPointerDown={down}
        onPointerMove={move}
        onPointerUp={up}
        onPointerCancel={reset}
        onContextMenu={(e) => e.preventDefault()}
      >
        <Square className="size-4 fill-current" />
        {ring > 0 && (
          <svg className="pointer-events-none absolute -inset-1 -rotate-90" viewBox={`0 0 ${RING} ${RING}`} aria-hidden>
            <circle
              cx={RING / 2}
              cy={RING / 2}
              r={(RING - STROKE) / 2}
              fill="none"
              stroke="currentColor"
              strokeWidth={STROKE}
              strokeDasharray={CIRC}
              strokeDashoffset={CIRC * (1 - ring)}
              strokeLinecap="round"
            />
          </svg>
        )}
      </Button>
    </>
  )
}

/** The button's top-centre in viewport space, for the Esc label when no hold anchored the pill. */
const at = (el: HTMLElement | null): { x: number; y: number } => {
  const r = el?.getBoundingClientRect()
  return r ? { x: r.left + r.width / 2, y: r.top } : { x: 0, y: 0 }
}
