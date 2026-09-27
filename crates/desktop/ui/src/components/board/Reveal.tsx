// Design 71 §2.8 / M7 — progressive reveal. Block ids unseen at the previous render rise in (CSS `board-rise`,
// 240 ms); a fresh `Graph`/`Sequence` staggers its units 40 ms apart in authored order (renderer-owned: the model
// inserts diagrams whole). The first render is the baseline, so reopening a finished board draws nothing. The
// drawing badge reads the `note-chat` binding → `sessions[session].running` — no lease, no activity RPC.
import { type HTMLAttributes, type ReactNode, useEffect, useRef, useState } from 'react'
import { useNoteChat } from '@/lib/note-chat'
import { useSessionField } from '@/store/sessions'
import type { DocBlock } from './blocks'

/** `risen`: every block id that arrived after the first render (sticky for the mount, so a rise is never cut short). */
export type Reveal = { risen: ReadonlySet<string>; last: string | null }

const NONE: ReadonlySet<string> = new Set()

/** Pure core: given the ids seen so far (mutated) and the current blocks, the fresh ids. `null` seen = first render. */
export function freshIds(seen: Set<string> | null, blocks: DocBlock[]): { seen: Set<string>; fresh: ReadonlySet<string> } {
  if (!seen) return { seen: new Set(blocks.map((b) => b.id)), fresh: NONE }
  const fresh = new Set<string>()
  for (const b of blocks) {
    if (seen.has(b.id)) continue
    seen.add(b.id)
    fresh.add(b.id)
  }
  return { seen, fresh: fresh.size ? fresh : NONE }
}

/** Which top-level blocks arrived since the first render, and the last one (for the badge's section). */
export function useReveal(blocks: DocBlock[]): Reveal {
  const state = useRef<{ seen: Set<string> | null; blocks: DocBlock[] | null; risen: ReadonlySet<string>; last: string | null }>({ seen: null, blocks: null, risen: NONE, last: null })
  const s = state.current
  // Idempotent per `blocks` identity (StrictMode renders twice with the same array).
  if (s.blocks !== blocks) {
    const r = freshIds(s.seen, blocks)
    s.seen = r.seen
    s.blocks = blocks
    if (r.fresh.size) {
      s.risen = new Set([...s.risen, ...r.fresh])
      s.last = blocks.findLast((b) => r.fresh.has(b.id))?.id ?? s.last
    }
  }
  return { risen: s.risen, last: s.last }
}

/** Unit stagger: 240 ms rise + `i × 40 ms` per unit, then the marker is dropped so a re-layout never replays it. */
export const RISE_MS = 240
export const STAGGER_MS = 40
const SETTLE_MS = 2000

/**
 * A fresh diagram block: `data-rise` on the frame and `--unit-i` on every unit (`[data-node]`, `[data-step]`) as the
 * renderer emits them (the `Graph` lays out through the 65 §4.3 queue, so units appear after mount). `rest` is the
 * caller's wrapper handlers (step 5's unit delegation) — one element owns `data-block`.
 */
export function StaggerFrame({ blockId, fresh, children, ...rest }: { blockId: string; fresh: boolean; children: ReactNode } & HTMLAttributes<HTMLDivElement>) {
  const ref = useRef<HTMLDivElement>(null)
  // Decided at mount: a frame that arrived fresh rises and staggers once, whatever later renders say.
  const [rising, setRising] = useState(fresh)
  useEffect(() => {
    const el = ref.current
    if (!rising || !el) return
    const index = () => {
      el.querySelectorAll<HTMLElement | SVGElement>('[data-node], [data-step]').forEach((u, i) => {
        if (!u.style.getPropertyValue('--unit-i')) u.style.setProperty('--unit-i', String(i))
      })
    }
    index()
    const mo = new MutationObserver(index)
    mo.observe(el, { childList: true, subtree: true })
    const t = window.setTimeout(() => {
      mo.disconnect()
      setRising(false)
    }, SETTLE_MS)
    return () => {
      mo.disconnect()
      clearTimeout(t)
    }
  }, [rising])
  return (
    <div ref={ref} {...rest} data-block={blockId} data-rise={rising || undefined}>
      {children}
    </div>
  )
}

/** "theuth is drawing · <section>" while the bound session runs (§0 Q5 binding, `sessions[session].running`). */
export function DrawingBadge({ doc, section }: { doc: string; section: string | null }) {
  const session = useNoteChat((s) => s.notes[doc]?.session ?? null)
  const running = useSessionField(session ?? '', 'running')
  if (!running) return null
  return (
    <div className="board-badge board-chrome" role="status" aria-live="polite">
      <span className="board-badge-dot" aria-hidden />
      theuth is drawing{section ? ` · ${section}` : ''}
    </div>
  )
}
