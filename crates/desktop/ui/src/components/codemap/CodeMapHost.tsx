/**
 * 73 §6.7 / §12.1 (Sky's ruling): the CodeMap is a real side pane docked beside the chat, not a Sheet overlay.
 * `CodeMapPane` wraps one chat leaf in a row flex and docks the panel on its right with a drag strip (360–920 px,
 * persisted); under 768 px `CodeMapSheet` (mounted from the header) shows the list forms in a bottom Sheet instead.
 * Availability = `codemap.roots` non-empty, exactly like `browser` / `terminal` (§0.2).
 */
import { MapIcon } from 'lucide-react'
import { type CSSProperties, type PointerEvent, type ReactNode, useEffect, useRef, useState } from 'react'
import { MenuRow } from '@/components/chat/HeaderMenu'
import { Button } from '@/components/ui/button'
import { Sheet, SheetClose, SheetContent, SheetTitle } from '@/components/ui/sheet'
import { useIsMobile } from '@/hooks/use-mobile'
import { useSessionField, useSessions } from '@/store/sessions'
import { CodeMapPanel } from './CodeMapPanel'

const WIDTH_KEY = 'codemap-panel-width'
const MIN_W = 360
const MAX_W = 920
const DEFAULT_W = 480

const clampW = (w: number) => Math.min(MAX_W, Math.max(MIN_W, w), Math.floor(window.innerWidth / 2))

export const useCodemapAvailable = (session: string | null): boolean =>
  (useSessionField(session ?? '', 'codemap')?.roots.length ?? 0) > 0

/** The header button: toggles the pane on the last touched root; a dot while a call arrived that the pane has not shown. */
export function CodeMapButton({ session, row }: { session: string; row?: boolean }) {
  const codemap = useSessionField(session, 'codemap')
  const open = useSessionField(session, 'codemapPanel') ?? null
  const seen = useRef<number>(-1)
  const last = codemap?.last ?? null
  if (open !== null && last) seen.current = last.seq
  const dot = open === null && last !== null && last.seq !== seen.current
  const toggle = () => {
    const root = last?.root ?? codemap?.roots.at(-1) ?? null
    useSessions.getState().setCodemapPanel(session, open === null ? root : null)
  }
  if (row) return <MenuRow icon={MapIcon} label={open === null ? 'CodeMap' : 'Close CodeMap'} onSelect={toggle} />
  return (
    <Button variant="ghost" className="relative size-9 rounded-lg text-fg-secondary" aria-label="CodeMap" aria-pressed={open !== null} onClick={toggle}>
      <MapIcon className="size-5" />
      {dot && <span aria-hidden className="absolute top-2 right-2 size-1.5 rounded-full bg-fg-accent" />}
    </Button>
  )
}

/** Docked host: `children` = the chat column; the panel sits to its right when `codemapPanel` names a root (desktop only). */
export function CodeMapPane({ session, children }: { session: string; children: ReactNode }) {
  const root = useSessionField(session, 'codemapPanel') ?? null
  const mobile = useIsMobile()
  const [width, setWidth] = useState(() => {
    const saved = Number(globalThis.localStorage?.getItem(WIDTH_KEY))
    return Number.isFinite(saved) && saved >= MIN_W ? saved : DEFAULT_W
  })
  useEffect(() => localStorage.setItem(WIDTH_KEY, String(width)), [width])
  const paneRef = useRef<HTMLDivElement>(null)
  // ponytail: plain pointer events on a 6 px strip, like ArtifactPanel/Panes; react-resizable-panels if a third pane ever needs it.
  const drag = (e: PointerEvent<HTMLDivElement>) => {
    const right = paneRef.current?.getBoundingClientRect().right ?? window.innerWidth
    e.preventDefault()
    e.currentTarget.setPointerCapture(e.pointerId)
    const move = (ev: globalThis.PointerEvent) => setWidth(clampW(right - ev.clientX))
    const up = () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', up)
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', up)
  }
  if (root === null || mobile) return <>{children}</>
  return (
    <div ref={paneRef} className="flex min-h-0 min-w-0 flex-1 flex-row">
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">{children}</div>
      <div role="separator" aria-orientation="vertical" onPointerDown={drag} className="z-10 w-1.5 shrink-0 cursor-col-resize bg-border-subtle hover:bg-fg-accent/60" />
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
