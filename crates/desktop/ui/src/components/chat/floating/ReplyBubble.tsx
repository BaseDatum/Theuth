// Shared glass reply bubble for floating chat docks (Whiteboard and Task Graph).
// Collapses to one line after 6 s; a tap expands it again. Offers optional Undo (whiteboards), Open session, Dismiss.

import { useEffect, useRef, useState } from 'react'
import { bubbleTimer, oneLine } from './useTurnReply'

export type ReplyBubbleProps = {
  text: string
  onDismiss(): void
  onOpenSession(): void
  /** Offer an undo button (e.g. whiteboard turn writes); omit when not applicable. */
  canUndo?: boolean
  undoing?: boolean
  onUndo?: () => void
  /** Positioned above the dock (not pinned to a card); `beside` shifts it left of a raised cluster. */
  floating?: boolean
  beside?: boolean
}

export function ReplyBubble({
  text,
  onDismiss,
  onOpenSession,
  canUndo = false,
  undoing = false,
  onUndo,
  floating = true,
  beside = false,
}: ReplyBubbleProps) {
  const [collapsed, setCollapsed] = useState(false)
  const timer = useRef(bubbleTimer(setCollapsed))
  useEffect(() => {
    const t = timer.current
    t.start()
    return () => t.dispose()
  }, [text])
  return (
    <div
      role="status"
      className={`wb-glass wb-rise w-[262px] rounded-2xl px-3 py-2.5 text-[12.5px] leading-[1.45] ${floating ? `absolute z-20 bottom-[var(--wb-dock,calc(80px+var(--safe-bottom,0px)))] ${beside ? 'right-[calc(94px+var(--safe-right,0px))]' : 'right-[calc(24px+var(--safe-right,0px))]'}` : ''}`}
      aria-label="The agent's reply"
      onPointerDown={(e) => e.stopPropagation()}
    >
      <button type="button" className="block w-full text-left" aria-expanded={!collapsed} onClick={() => timer.current.expand()}>
        {collapsed ? <span className="block truncate">{oneLine(text)}</span> : <span className="line-clamp-[10] whitespace-pre-wrap">{text}</span>}
      </button>
      {!collapsed && (
        <div className="mt-2 flex flex-wrap gap-2">
          {canUndo && onUndo && (
            <button type="button" disabled={undoing} onClick={onUndo} className="wb-well rounded-[10px] px-2.5 py-1.5 text-[11px] font-semibold disabled:opacity-50">
              {undoing ? 'Undoing…' : 'Undo this turn'}
            </button>
          )}
          <button type="button" onClick={onOpenSession} className="wb-well rounded-[10px] px-2.5 py-1.5 text-[11px] font-semibold">
            Open session
          </button>
          <button type="button" onClick={onDismiss} className="wb-well rounded-[10px] px-2.5 py-1.5 text-[11px] font-semibold">
            Dismiss
          </button>
        </div>
      )}
    </div>
  )
}
