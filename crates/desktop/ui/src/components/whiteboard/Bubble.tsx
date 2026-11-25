// 218 §8.8 "Reply bubble" / design §4.5: the bound session's reply after a turn, glass, pinned to the one card the turn
// touched (through the canvas's `anchored` prop) or above the pill. Collapses to one line after 6 s; a tap opens it
// again. Undo this turn and Dismiss.

import { useEffect, useRef, useState } from 'react'
import { bubbleTimer, oneLine } from './reply'

export type BubbleProps = {
  text: string
  /** Undo this turn is offered (the turn wrote to this whiteboard and is not undone). */
  canUndo: boolean
  undoing: boolean
  onUndo(): void
  onDismiss(): void
  onOpenSession(): void
  /** Above the pill (not pinned to a card); `beside` moves it left of a raised cluster. */
  floating: boolean
  beside?: boolean
}

export function Bubble(p: BubbleProps) {
  const [collapsed, setCollapsed] = useState(false)
  const timer = useRef(bubbleTimer(setCollapsed))
  useEffect(() => {
    const t = timer.current
    t.start()
    return () => t.dispose()
  }, [p.text])
  return (
    <div
      role="status"
      className={`wb-glass wb-rise w-[262px] rounded-2xl px-3 py-2.5 text-[12.5px] leading-[1.45] ${p.floating ? `absolute z-20 bottom-[calc(80px+var(--safe-bottom,0px))] ${p.beside ? 'right-[calc(94px+var(--safe-right,0px))]' : 'right-[calc(24px+var(--safe-right,0px))]'}` : ''}`}
      aria-label="The agent's reply"
      onPointerDown={(e) => e.stopPropagation()}
    >
      <button type="button" className="block w-full text-left" aria-expanded={!collapsed} onClick={() => timer.current.expand()}>
        {collapsed ? <span className="block truncate">{oneLine(p.text)}</span> : <span className="line-clamp-[10] whitespace-pre-wrap">{p.text}</span>}
      </button>
      {!collapsed && (
        <div className="mt-2 flex flex-wrap gap-2">
          {p.canUndo && (
            <button type="button" disabled={p.undoing} onClick={p.onUndo} className="wb-chip rounded-[10px] px-2.5 py-1.5 text-[11px] font-semibold disabled:opacity-50">
              {p.undoing ? 'Undoing…' : 'Undo this turn'}
            </button>
          )}
          <button type="button" onClick={p.onOpenSession} className="wb-chip rounded-[10px] px-2.5 py-1.5 text-[11px] font-semibold">
            Open session
          </button>
          <button type="button" onClick={p.onDismiss} className="wb-chip rounded-[10px] px-2.5 py-1.5 text-[11px] font-semibold">
            Dismiss
          </button>
        </div>
      )}
    </div>
  )
}
