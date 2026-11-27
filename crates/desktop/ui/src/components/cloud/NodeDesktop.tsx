// 224: a cloud node's watchable desktop — the noVNC session (`vm/rfb.tsx`, shared with the sandbox VM viewer) on the
// `cloud_desktop` grant. Watching by default; **Take over** only turns this viewer's input on (no agent steering on a
// cloud node), **Hand back** turns it off. A Dialog on desktop, a full-height bottom Sheet on the phone.

import { Maximize2, Minimize2, RotateCcw } from 'lucide-react'
import { type ReactNode, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { Sheet, SheetContent, SheetDescription, SheetTitle } from '@/components/ui/sheet'
import { cn } from '@/lib/utils'
import { SendKeysMenu, useRfb } from '@/components/vm/rfb'
import type { NodeDesktop } from './types'

export function NodeDesktopView({ name, access, placeholder }: { name: string; access: NodeDesktop | null; /** Fixture only: drawn instead of a VNC session. */ placeholder?: ReactNode }) {
  const [taken, setTaken] = useState(false)
  const [fit, setFit] = useState(true)
  const { box, rfb, conn, reason, geometry, desktopName, reconnect } = useRfb(placeholder ? null : access, !taken, fit)
  const shown = placeholder ? 'connected' : conn
  return (
    <div className="flex h-full min-h-0 flex-1 flex-col overflow-hidden rounded-lg bg-black">
      <div className="flex h-10 shrink-0 items-center gap-2 bg-elevated px-2 text-xs text-fg-secondary">
        <span className="font-medium text-foreground">{name}</span>
        {(geometry || access?.geometry) && <span className="tabular-nums">{geometry || access?.geometry}</span>}
        {desktopName && <span className="truncate text-fg-tertiary max-md:hidden">{desktopName}</span>}
        <span className={cn('rounded-full px-1.5', shown === 'connected' ? 'bg-success-bg text-success' : shown === 'failed' ? 'bg-danger-bg text-danger' : 'bg-warning-bg text-warning')}>
          {shown === 'connected' ? (taken ? 'taken over' : 'watching') : shown}
        </span>
        <span className="ml-auto flex items-center gap-1">
          <Button size="xs" variant={taken ? 'destructive' : 'outline'} onClick={() => setTaken((t) => !t)}>
            {taken ? 'Hand back' : 'Take over'}
          </Button>
          <SendKeysMenu rfb={rfb} disabled={!taken || shown !== 'connected'} />
          <Button size="icon-xs" variant="ghost" aria-label={fit ? 'Actual size' : 'Fit to window'} title={fit ? 'Actual size' : 'Fit to window'} onClick={() => setFit((f) => !f)}>
            {fit ? <Minimize2 /> : <Maximize2 />}
          </Button>
          <Button size="icon-xs" variant="ghost" aria-label="Reconnect" title="Reconnect" onClick={reconnect}>
            <RotateCcw />
          </Button>
        </span>
      </div>
      <div className={cn('relative min-h-0 flex-1', fit ? 'overflow-hidden' : 'overflow-auto', taken && 'ring-4 ring-danger ring-inset')}>
        {placeholder ?? <div ref={box} className={cn('h-full w-full', !fit && 'h-auto w-auto')} />}
        {shown !== 'connected' && (
          <div className="pointer-events-none absolute inset-0 flex items-center justify-center text-sm text-fg-secondary">{shown === 'reconnecting' ? 'reconnecting…' : (reason ?? 'connecting…')}</div>
        )}
      </div>
    </div>
  )
}

export function NodeDesktopPanel({ name, access, mobile, onClose, placeholder }: { name: string; access: NodeDesktop | null; mobile: boolean; onClose: () => void; placeholder?: ReactNode }) {
  const view = <NodeDesktopView name={name} access={access} placeholder={placeholder} />
  if (mobile)
    return (
      <Sheet open onOpenChange={(o) => !o && onClose()}>
        <SheetContent side="bottom" className="h-[100dvh] gap-2 p-2 pt-[calc(var(--safe-top)+2.5rem)]">
          <SheetTitle className="sr-only">{name} · desktop</SheetTitle>
          <SheetDescription className="sr-only">The cloud node's watchable desktop.</SheetDescription>
          {view}
        </SheetContent>
      </Sheet>
    )
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="flex h-[80dvh] flex-col gap-3 sm:max-w-5xl">
        <DialogTitle>{name} · desktop</DialogTitle>
        <DialogDescription className="sr-only">The cloud node's watchable desktop.</DialogDescription>
        {view}
      </DialogContent>
    </Dialog>
  )
}
