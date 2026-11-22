import { useNavigate } from '@tanstack/react-router'
import { type ReactNode, type RefObject, useEffect, useRef } from 'react'
import { ResizeHandle } from '@/components/ui/resize-handle'
import { useSidebar } from '@/components/ui/sidebar'
import { usePressDrag } from '@/hooks/use-press-drag'
import { TileHost } from '@/components/tiles/host'
import { type Box, isDraft, isFrame, layout, type Layout, leaves, newDraft, shown, usePanes, zone } from '@/lib/panes'
import { closeAllTiles } from '@/lib/tiles'
import { clampRatio, RATIO_MAX, RATIO_MIN, RATIO_STEP } from '@/lib/resize-drag'
import { cn } from '@/lib/utils'

/**
 * Drag a sidebar row onto a chat pane (`usePressDrag` owns the gesture and its DOM glue). Spread the result on the
 * row's `Link`; a press that became a drag never navigates, selects or opens the row's context menu. `id === null`
 * is the New chat rail item: each drag carries a fresh draft id, and a drop opens a draft pane (`NewChatPane`)
 * without moving the URL. In the phone Sheet fingers never drag: phones do not tile.
 */
export function useSessionDrag(id: string | null, title: string) {
  const navigate = useNavigate()
  const { isMobile } = useSidebar()
  return usePressDrag({
    disabledForFinger: isMobile,
    onStart: (x, y) => usePanes.getState().startDrag(id ?? newDraft(), title, x, y),
    onMove: (x, y) => usePanes.getState().moveDrag(x, y),
    onEnd(drop) {
      const dropped = usePanes.getState().endDrag(drop)
      if (dropped && !isDraft(dropped)) void navigate({ to: '/c/$session', params: { session: dropped } })
    },
  }).props
}

/** The drag ghost and the snapped drop zone; mounted once in the shell, renders nothing between drags. */
export function DragOverlay() {
  const drag = usePanes((s) => s.drag)
  if (!drag) return null
  return (
    <>
      {drag.target && (
        <div
          className="pointer-events-none fixed z-50 rounded-xl border-2 border-fg-accent bg-fg-accent/10 transition-[left,top,width,height] duration-100"
          style={zone(drag.target)}
        />
      )}
      <div
        className="pointer-events-none fixed z-50 max-w-56 truncate rounded-md border bg-popover px-2 py-1 text-xs shadow-md"
        style={{ left: drag.x + 12, top: drag.y + 12 }}
      >
        {drag.title}
      </div>
    </>
  )
}

/** One leaf: the drop target for the pane it shows. A press inside selects it (`usePanes.selected`, any kind) and, for a
 * chat, focuses it and moves the URL there (a draft has no URL); a frame (a web view, 205 D5, or a generic tile) is
 * selected only — the URL stays on the chat. The selected pane is lit, the others dim. A press on a control in a chat
 * while a frame is selected changes nothing, so a surface that control opens takes over the selected frame's pane.
 * `tiles`: the layout tiles (not a phone), so the subtree's surfaces open as tiles beside this leaf (`useTileHost`).
 * `box`: where it sits in the panes area, in percent. */
function Leaf({ session, first, tiles, box, render }: { session: string; first: boolean; tiles: boolean; box: Box; render(session: string, first: boolean): ReactNode }) {
  const navigate = useNavigate()
  const focused = usePanes((s) => s.focused)
  const lit = usePanes((s) => (s.selected !== null && s.root !== null && leaves(s.root).includes(s.selected) ? s.selected : s.focused))
  const split = usePanes((s) => s.root !== null)
  return (
    <div
      data-pane={session}
      className={cn('absolute flex min-h-0 min-w-0 flex-col', split && lit !== session && 'opacity-80')}
      style={{ left: `${box.left}%`, top: `${box.top}%`, width: `${box.width}%`, height: `${box.height}%` }}
      onPointerDownCapture={(e) => {
        const { root, selected, select, focus } = usePanes.getState()
        const control = e.target instanceof Element && e.target.closest('button,a,[role=button],[role=menuitem]') !== null
        const frameLit = isFrame(selected) && selected !== session && root !== null && leaves(root).includes(selected)
        if (control && !isFrame(session) && frameLit) return
        select(session)
        if (focused === session || isFrame(session)) return
        focus(session)
        if (!isDraft(session)) void navigate({ to: '/c/$session', params: { session } })
      }}
    >
      <TileHost.Provider value={tiles ? session : null}>{render(session, first)}</TileHost.Provider>
    </div>
  )
}

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

/** The divider of one split, laid over the line between its halves; the drag measures the split's own region of the
 * panes `area` and reports the new first-half share to `usePanes.resize`. */
function SplitDivider({ split: s, area }: { split: Layout['splits'][number]; area: RefObject<HTMLDivElement | null> }) {
  const region = useRef<{ left: number; top: number; width: number; height: number }>(null)
  const row = s.dir === 'row'
  const onRatio = (r: number) => usePanes.getState().resize(s.path, r)
  return (
    <div
      className={cn('absolute z-10 flex', row ? '-translate-x-1/2 flex-row' : '-translate-y-1/2 flex-col')}
      style={
        row
          ? { left: `${s.box.left + s.box.width * s.ratio}%`, top: `${s.box.top}%`, height: `${s.box.height}%` }
          : { top: `${s.box.top + s.box.height * s.ratio}%`, left: `${s.box.left}%`, width: `${s.box.width}%` }
      }
    >
      <ResizeHandle
        orientation={row ? 'vertical' : 'horizontal'}
        value={{ now: Math.round(s.ratio * 100), min: RATIO_MIN * 100, max: RATIO_MAX * 100 }}
        onStart={() => {
          const a = area.current?.getBoundingClientRect()
          region.current = a
            ? { left: a.left + (a.width * s.box.left) / 100, top: a.top + (a.height * s.box.top) / 100, width: (a.width * s.box.width) / 100, height: (a.height * s.box.height) / 100 }
            : null
        }}
        onMove={(x, y) => {
          const r = region.current
          if (r) onRatio(clampRatio(row ? (x - r.left) / r.width : (y - r.top) / r.height))
        }}
        onNudge={(d) => onRatio(clampRatio(s.ratio + d * RATIO_STEP))}
      />
    </div>
  )
}

/**
 * The chat area: the split tree when there is one, else the route's single chat (still a drop target). Every leaf is
 * one child of the same box, keyed by its leaf string and placed by `layout`, so opening, moving or closing a tile never
 * remounts another pane (a terminal, a VNC session, a composer keep their state); dividers are laid over the lines.
 * A phone (`isMobile`) does not tile (25 §12.3): whatever the tree holds, the focused leaf alone fills the screen as
 * `first`, so its header always carries the sidebar toggle — the Sheet's only opener. Crossing into the phone layout
 * closes the generic tiles (their surfaces open as sheets there), so widening again shows a consistent tree.
 */
export function Panes({ session, render }: { session: string; render(session: string, first: boolean): ReactNode }) {
  const root = usePanes((s) => s.root)
  const focused = usePanes((s) => s.focused)
  const { isMobile } = useSidebar()
  const area = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (isMobile) closeAllTiles()
  }, [isMobile])
  const full: Box = { left: 0, top: 0, width: 100, height: 100 }
  const { leaves, splits } = isMobile ? { leaves: [{ session: shown(root, focused, session), box: full, first: true }], splits: [] } : layout(root ?? { session })
  return (
    <div ref={area} className="relative min-h-0 min-w-0 flex-1">
      {leaves.map((l) => (
        <Leaf key={l.session} session={l.session} first={l.first} tiles={!isMobile} box={l.box} render={render} />
      ))}
      {splits.map((s) => (
        <SplitDivider key={`split:${s.path.join('')}`} split={s} area={area} />
      ))}
    </div>
  )
}
