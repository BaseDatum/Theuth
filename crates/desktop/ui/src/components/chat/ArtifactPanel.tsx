import { X } from 'lucide-react'
import { type CSSProperties, type PointerEvent, useEffect, useMemo, useRef, useState } from 'react'
import { ArtifactView } from '@/components/artifacts/Viewer'
import { Button } from '@/components/ui/button'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Sheet, SheetClose, SheetContent, SheetDescription, SheetTitle } from '@/components/ui/sheet'
import { fmtBytes } from '@/lib/files'
import { on, useSessionField, useSessions } from '@/store/sessions'
import { ArtifactMenu, RevealButton } from './ArtifactCard'

const MIN_W = 360
const DEFAULT_W = 640

/** Header + body for the artifact at `callId`; a version `Select` swaps the viewer among same-name artifacts of the session. */
function Content({ session, callId }: { session: string; callId: string }) {
  const artifacts = useSessionField(session, 'artifacts')
  const [view, setView] = useState(callId)
  const shown = artifacts?.get(view) ?? artifacts?.get(callId)
  const versions = useMemo(() => {
    if (!shown || !artifacts) return []
    return [...artifacts.values()].filter((a) => a.name === shown.name).sort((x, y) => y.version - x.version)
  }, [artifacts, shown])
  if (!shown) return null
  return (
    <>
      <div className="flex items-start gap-2 border-b border-border-subtle px-4 pt-3 pb-3 pr-3">
        <div className="min-w-0 flex-1">
          <SheetTitle className="truncate text-base">{shown.title}</SheetTitle>
          <SheetDescription className="truncate text-xs">
            {shown.name} · {shown.kind.toUpperCase()} · {fmtBytes(shown.bytes)}
          </SheetDescription>
        </div>
        {versions.length > 1 && (
          <Select value={shown.call_id} onValueChange={setView}>
            <SelectTrigger size="sm" className="h-8 shrink-0 text-xs" aria-label="Version">
              <SelectValue />
            </SelectTrigger>
            <SelectContent align="end">
              {versions.map((v) => (
                <SelectItem key={v.call_id} value={v.call_id} className="text-xs">
                  v{v.version}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}
        <RevealButton artifact={shown} />
        <ArtifactMenu artifact={shown} />
        <SheetClose asChild>
          <Button variant="ghost" size="icon-sm" className="text-fg-secondary" aria-label="Close">
            <X className="size-4" />
          </Button>
        </SheetClose>
      </div>
      <ArtifactView artifact={shown} className="min-h-0 flex-1 overflow-auto" />
    </>
  )
}

/**
 * Right-side sheet showing one artifact (35 §7.2). Opens itself for the first
 * `ArtifactShown` of a turn while the window is focused; closing it suppresses
 * auto-open for the rest of that turn.
 */
export function ArtifactPanel({ session }: { session: string }) {
  const callId = useSessionField(session, 'artifactPanel') ?? null
  const [width, setWidth] = useState(DEFAULT_W)
  /** `turnSeq` of the turn whose first artifact already opened (or was dismissed). */
  const handled = useRef<number | undefined>(undefined)

  useEffect(
    () =>
      on('ArtifactShown', (e) => {
        if (e.session !== session) return
        const turn = useSessions.getState().sessions[session]?.turnSeq
        if (handled.current === turn) return
        handled.current = turn
        if (document.hasFocus()) useSessions.getState().setArtifactPanel(session, e.call_id)
      }),
    [session],
  )

  const close = () => {
    handled.current = useSessions.getState().sessions[session]?.turnSeq
    useSessions.getState().setArtifactPanel(session, null)
  }

  // ponytail: plain pointer events on a 6 px strip; react-resizable-panels if a second panel ever needs it.
  const drag = (e: PointerEvent<HTMLDivElement>) => {
    e.currentTarget.setPointerCapture(e.pointerId)
    const move = (ev: globalThis.PointerEvent) =>
      setWidth(Math.min(Math.max(MIN_W, window.innerWidth - ev.clientX), window.innerWidth / 2))
    const up = () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', up)
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', up)
  }

  return (
    <Sheet open={callId !== null} onOpenChange={(o) => !o && close()}>
      <SheetContent
        showCloseButton={false}
        className="w-full gap-0 p-0 transition-transform sm:w-(--panel-w) sm:max-w-[50vw]"
        style={{ '--panel-w': `${width}px` } as CSSProperties}
      >
        <div
          className="absolute inset-y-0 left-0 z-10 hidden w-1.5 cursor-col-resize hover:bg-border-strong sm:block"
          onPointerDown={drag}
        />
        {callId && <Content key={callId} session={session} callId={callId} />}
      </SheetContent>
    </Sheet>
  )
}
