/**
 * 73 §6.7 / §12.1 (Sky's ruling): the CodeMap is a real side pane beside the chat, not a Sheet overlay. In the pane
 * tree it is the `codemap` tile (`components/tiles/kinds/codemap`); the talk view docks it with `CodeMapPane` (a drag
 * strip, 360–920 px, persisted); under 768 px `CodeMapSheet` (mounted from the header) shows the list forms in a bottom
 * Sheet instead. Availability = `codemap.roots` non-empty, exactly like `browser` / `terminal` (§0.2).
 */
import { useSupported } from '@/rpc/capabilities'
import { requirements } from './requirements'
import { type CSSProperties, type ReactNode, useEffect, useRef, useState } from 'react'
import { cn } from '@/lib/utils'
import { useTileHost } from '@/components/tiles/host'
import { openCodemapTile } from '@/components/tiles/open'
import { ResizeHandle } from '@/components/ui/resize-handle'
import { Sheet, SheetClose, SheetContent, SheetTitle } from '@/components/ui/sheet'
import { useIsMobile } from '@/hooks/use-mobile'
import { PX_STEP } from '@/lib/resize-drag'
import { closeTile, tileId, useTileOpen } from '@/lib/tiles'
import { useSessionField, useSessions } from '@/store/sessions'
import { CodeMapPanel } from './CodeMapPanel'
import { usePayloadDrag } from '@/components/chat/Panes'
import type { TrayDrag } from '@/components/chat/tray-drag'

const WIDTH_KEY = 'codemap-panel-width'
const MIN_W = 360
const MAX_W = 920
const DEFAULT_W = 480

const clampW = (w: number) => Math.min(MAX_W, Math.max(MIN_W, w), Math.floor(window.innerWidth / 2))

export const useCodemapAvailable = (session: string | null): boolean =>
  (useSessionField(session ?? '', 'codemap')?.roots.length ?? 0) > 0

/**
 * The tray's Code map tile (`children` in a plain button styled by `className`): toggles the CodeMap on the last touched
 * root — the tile when the chat sits in a pane, else the docked pane / phone Sheet (`codemapPanel`); a dot while a call
 * arrived that the CodeMap has not shown.
 */
export function CodeMapButton({ session, className, drag, children }: { session: string; className?: string; drag?: TrayDrag; children: ReactNode }) {
  const host = useTileHost()
  const dragProps = usePayloadDrag(drag?.payload ?? null, drag?.title ?? '')
  const codemap = useSessionField(session, 'codemap')
  const panel = useSessionField(session, 'codemapPanel') ?? null
  const id = tileId('codemap', session)
  const inTree = useTileOpen('codemap', session)
  const open = host ? inTree : panel !== null
  const seen = useRef<number>(-1)
  const last = codemap?.last ?? null
  if (open && last) seen.current = last.seq
  const dot = !open && last !== null && last.seq !== seen.current
  const toggle = () => {
    const root = last?.root ?? codemap?.roots.at(-1) ?? null
    if (host) {
      if (open) closeTile(id)
      else if (root !== null) openCodemapTile(host, root)
    } else useSessions.getState().setCodemapPanel(session, open ? null : root)
  }
  return (
    <button type="button" className={cn('relative', className)} aria-label={open ? 'Close CodeMap' : 'CodeMap'} aria-pressed={open} onClick={toggle} {...dragProps}>
      {children}
      {dot && <span aria-hidden className="absolute right-2.5 bottom-2.5 size-1.5 rounded-full bg-fg-accent" />}
    </button>
  )
}

/** Docked host outside the pane tree (the talk view): `children` = the chat column; the panel sits to its right when `codemapPanel` names a root (desktop only). */
export function CodeMapPane({ session, children }: { session: string; children: ReactNode }) {
  const root = useSessionField(session, 'codemapPanel') ?? null
  const available = useSupported(requirements)
  const mobile = useIsMobile()
  const [width, setWidth] = useState(() => {
    const saved = Number(globalThis.localStorage?.getItem(WIDTH_KEY))
    return Number.isFinite(saved) && saved >= MIN_W ? saved : DEFAULT_W
  })
  useEffect(() => localStorage.setItem(WIDTH_KEY, String(width)), [width])
  const paneRef = useRef<HTMLDivElement>(null)
  const right = useRef(0)
  if (root === null || mobile || !available) return <>{children}</>
  return (
    <div ref={paneRef} className="flex min-h-0 min-w-0 flex-1 flex-row">
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">{children}</div>
      <ResizeHandle
        orientation="vertical"
        aria-label="Resize CodeMap pane"
        className="w-1.5"
        onStart={() => (right.current = paneRef.current?.getBoundingClientRect().right ?? window.innerWidth)}
        onMove={(x) => setWidth(clampW(right.current - x))}
        onNudge={(d) => setWidth(clampW(width - d * PX_STEP))}
      />
      <aside
        data-codemap-pane
        className="flex min-h-0 shrink-0 flex-col overflow-hidden border-l border-border-subtle bg-layer-1"
        style={{ width: `min(var(--codemap-w), 50vw)`, '--codemap-w': `${width}px` } as CSSProperties}
      >
        <CodeMapPanel key={`${session}:${root}`} session={session} root={root} onClose={() => useSessions.getState().setCodemapPanel(session, null)} />
      </aside>
    </div>
  )
}

/** Phones: the list forms in a full-height bottom Sheet (the `TerminalViewer` pattern); mounted from the header. */
export function CodeMapSheet({ session }: { session: string }) {
  const root = useSessionField(session, 'codemapPanel') ?? null
  const mobile = useIsMobile()
  if (!mobile) return null
  const close = () => useSessions.getState().setCodemapPanel(session, null)
  return (
    <Sheet open={root !== null} onOpenChange={(o) => !o && close()}>
      <SheetContent side="bottom" showCloseButton={false} className="top-0 h-band gap-0 pt-[var(--safe-top)]">
        <SheetTitle className="sr-only">CodeMap</SheetTitle>
        <SheetClose className="sr-only">Close</SheetClose>
        {root !== null && <CodeMapPanel key={`${session}:${root}`} session={session} root={root} onClose={close} />}
      </SheetContent>
    </Sheet>
  )
}
