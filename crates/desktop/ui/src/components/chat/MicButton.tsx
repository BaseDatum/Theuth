// 89 VOICE-HOLD (amends 76 §0.12): the composer's microphone as one gesture control; reducer in `lib/hold.ts`, the
// floating layer and row pieces in `VoiceHold.tsx`. Tap → dictation. Press → the button swells; at `HOLD_MS` a
// voice note records and the orb + lock pill float over the page while the finger is down. Slide up → lock
// hands-free (the row becomes Delete · timer · wave · Send); slide left → cancel; release anywhere → send.
// Pointer events with capture, so the finger is followed wherever it goes.

import { MicGlyph } from '@/components/voice/VoiceIcons'
import { type PointerEvent as ReactPointerEvent, type RefObject, useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import type { FrameSource, VisualFrame } from '@/lib/audio/types'
import { haptics } from '@/lib/haptics'
import { type Act, CANCEL_PX, HOLD_MS, type Hold, IDLE, step, type Verb } from '@/lib/hold'
import { cn } from '@/lib/utils'
import { type Anchor, type Exit, type LayerHandle, SendNote, type Stage, VoiceLayer, ms } from './VoiceHold'

export function MicButton({
  recording,
  disabled,
  reason,
  source,
  hint,
  onTap,
  onRecord,
  onStop,
  onStage,
}: {
  recording: boolean
  disabled: boolean
  /** Why dictation cannot start now (the tooltip); the hold still records. */
  reason: string | null
  /** The recorder's analyser, for the orb's halo. */
  source: FrameSource<VisualFrame> | null
  /** The composer's "‹ slide to cancel", moved and faded with the finger. */
  hint: RefObject<HTMLSpanElement | null>
  onTap: () => void
  onRecord: () => void
  onStop: (discard: boolean) => void
  /** What the composer row should show. */
  onStage: (stage: Stage) => void
}) {
  const [hold, setHold] = useState<Hold>(IDLE)
  const [layer, setLayer] = useState<{ anchor: Anchor; exit: Exit } | null>(null)
  /** The 420 ms bin wiggle after a slide-left; the reducer is already idle. */
  const [cancelled, setCancelled] = useState(false)
  const ref = useRef<HTMLButtonElement>(null)
  const fx = useRef<LayerHandle>(null)
  const timer = useRef(0)
  const leaving = useRef(0)
  const wiggle = useRef(0)
  const state = useRef(hold)
  state.current = hold
  const stageCb = useRef(onStage)
  stageCb.current = onStage

  const stage: Stage = hold.phase === 'held' || hold.phase === 'locked' ? hold.phase : cancelled ? 'cancelled' : 'idle'
  useEffect(() => stageCb.current(stage), [stage])

  /** The finger's travel, straight to the DOM: the pill and lock icon (layer), the cancel hint (composer row). */
  const paint = (h: Hold) => {
    if (h.phase !== 'held') return
    fx.current?.track(h)
    const el = hint.current
    if (!el) return
    const left = Math.max(0, -h.dx)
    el.style.transform = `translateX(${-left}px)`
    el.style.opacity = String(Math.max(0, 1 - left / CANCEL_PX))
  }
  const leave = (exit: Exit, after: number) => {
    setLayer((l) => l && { ...l, exit })
    clearTimeout(leaving.current)
    leaving.current = window.setTimeout(() => setLayer(null), ms(after))
  }
  const run = (verb: Verb, prev: Hold) => {
    if (verb === 'tap') onTap()
    else if (verb === 'record') {
      haptics.tick()
      const r = ref.current?.getBoundingClientRect()
      if (r) setLayer({ anchor: { x: r.left + r.width / 2, y: r.top + r.height / 2 }, exit: null })
      onRecord()
    } else if (verb === 'lock') {
      haptics.lock()
      leave('lock', 400)
    } else if (verb === 'send') {
      haptics.send()
      leave('send', 160)
      onStop(false)
    } else if (verb === 'cancel') {
      haptics.cancel()
      leave('cancel', 200)
      const el = hint.current
      if (el) {
        el.style.transition = `transform ${ms(150)}ms ease-in, opacity ${ms(150)}ms ease-in`
        el.style.transform = `translateX(${(prev.phase === 'held' ? prev.dx : 0) - 40}px)`
        el.style.opacity = '0'
      }
      setCancelled(true)
      clearTimeout(wiggle.current)
      wiggle.current = window.setTimeout(() => setCancelled(false), ms(420))
      onStop(true)
    }
  }
  const act = (a: Act) => {
    const prev = state.current
    const [next, verb] = step(prev, a)
    state.current = next
    setHold(next)
    paint(next)
    run(verb, prev)
  }

  // The recording ended by any path (send, cancel, Escape, a failed start): the control is a mic again; a layer
  // that is not already animating out goes at once.
  useEffect(() => {
    if (recording) return
    if (state.current.phase !== 'pressed') act({ type: 'reset' })
    setLayer((l) => (l?.exit ? l : null))
  }, [recording])
  useEffect(
    () => () => {
      clearTimeout(timer.current)
      clearTimeout(leaving.current)
      clearTimeout(wiggle.current)
    },
    [],
  )

  const held = hold.phase === 'held'
  const locked = hold.phase === 'locked'

  // While held, losing the pointer sequence (the window blurring, the page hidden, `pointercancel` below) locks:
  // the note goes on hands-free and the user decides afterwards.
  useEffect(() => {
    if (!held) return
    const interrupt = () => act({ type: 'interrupt' })
    const hidden = () => document.visibilityState === 'hidden' && interrupt()
    window.addEventListener('blur', interrupt)
    document.addEventListener('visibilitychange', hidden)
    return () => {
      window.removeEventListener('blur', interrupt)
      document.removeEventListener('visibilitychange', hidden)
    }
  }, [held])
  // The page under the gesture: no selection, no rubber-banding.
  useEffect(() => {
    if (!held && !locked) return
    document.body.classList.add('voice-hold')
    return () => document.body.classList.remove('voice-hold')
  }, [held || locked])
  // The layer follows the button if the page moves under it (the keyboard, a resize).
  useEffect(() => {
    if (!layer) return
    const place = () => {
      const r = ref.current?.getBoundingClientRect()
      if (r) setLayer((l) => l && { ...l, anchor: { x: r.left + r.width / 2, y: r.top + r.height / 2 } })
    }
    window.addEventListener('resize', place)
    window.addEventListener('scroll', place, true)
    return () => {
      window.removeEventListener('resize', place)
      window.removeEventListener('scroll', place, true)
    }
  }, [!!layer])

  const down = (e: ReactPointerEvent<HTMLButtonElement>) => {
    if (disabled || e.button !== 0) return
    e.currentTarget.setPointerCapture(e.pointerId)
    clearTimeout(timer.current)
    timer.current = window.setTimeout(() => act({ type: 'timer' }), HOLD_MS)
    haptics.tick()
    act({ type: 'down', x: e.clientX, y: e.clientY })
  }
  const move = (e: ReactPointerEvent<HTMLButtonElement>) => act({ type: 'move', x: e.clientX, y: e.clientY })
  const up = (e: ReactPointerEvent<HTMLButtonElement>) => {
    clearTimeout(timer.current)
    act({ type: 'up', x: e.clientX, y: e.clientY })
  }
  const cancel = () => {
    clearTimeout(timer.current)
    act({ type: 'interrupt' })
  }

  return (
    <>
      {layer && <VoiceLayer ref={fx} anchor={layer.anchor} exit={layer.exit} source={source} />}
      {locked ? (
        // Its own node (`key`): the pointer sequence that locked ends in a `click`, which must not land on Send.
        <SendNote key="send" ref={ref} disabled={disabled} onClick={() => onStop(false)} />
      ) : (
        <Tooltip key="mic">
          <TooltipTrigger asChild>
            <Button
              ref={ref}
              variant="ghost"
              // `data-drag-row` (styles.css): no text selection or iOS callout under a held finger.
              data-drag-row
              className={cn(
                'relative size-9 shrink-0 rounded-full select-none touch-none duration-120 ease-out motion-reduce:transition-none',
                hold.phase === 'pressed' && 'scale-[1.12]',
                !held && reason !== null && 'opacity-50',
              )}
              aria-label={held ? 'Recording — release to send' : 'Microphone'}
              aria-disabled={!held && reason !== null}
              disabled={disabled}
              onPointerDown={down}
              onPointerMove={move}
              onPointerUp={up}
              onPointerCancel={cancel}
              onContextMenu={(e) => e.preventDefault()}
            >
              <MicGlyph className="size-5" />
            </Button>
          </TooltipTrigger>
          <TooltipContent>{reason ?? 'Dictation (Ctrl+D) · hold to record a voice note · slide up to lock, left to cancel'}</TooltipContent>
        </Tooltip>
      )}
    </>
  )
}
