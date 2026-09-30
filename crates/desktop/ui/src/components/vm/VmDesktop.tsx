import RFB from '@novnc/novnc'
import { useQuery } from '@tanstack/react-query'
import { Keyboard, Maximize2, Minimize2, Play, RotateCcw } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { toast } from 'sonner'
import { useHandoffs, useSandboxActions, useSandboxes, unresolved } from '@/components/settings/sandbox-queries'
import { useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { HANDOFFS_KEY, stateLabel } from '@/store/sandboxes'
import { on } from '@/store/sessions'

// X11 keysyms (noVNC's `core/input/keysym.js` is not on the package's export map).
const XK = { Control_L: 0xffe3, Alt_L: 0xffe9, Super_L: 0xffeb, Tab: 0xff09, F4: 0xffc1, Escape: 0xff1b, Print: 0xff61 }
const CHORDS: readonly [string, (r: RFB) => void][] = [
  ['Ctrl-Alt-Del', (r) => r.sendCtrlAltDel()],
  ['Alt-Tab', (r) => chord(r, [[XK.Alt_L, 'AltLeft']], [XK.Tab, 'Tab'])],
  ['Alt-F4', (r) => chord(r, [[XK.Alt_L, 'AltLeft']], [XK.F4, 'F4'])],
  ['Super (app menu)', (r) => r.sendKey(XK.Super_L, 'MetaLeft')],
  ['Escape', (r) => r.sendKey(XK.Escape, 'Escape')],
  ['Print Screen', (r) => r.sendKey(XK.Print, 'PrintScreen')],
]
function chord(r: RFB, mods: [number, string][], key: [number, string]) {
  for (const [k, c] of mods) r.sendKey(k, c, true)
  r.sendKey(key[0], key[1])
  for (const [k, c] of mods.reverse()) r.sendKey(k, c, false)
}

type Conn = 'connecting' | 'connected' | 'reconnecting' | 'failed'
const RETRY_MS = [1000, 2000, 4000, 8000, 15000]

/**
 * `/vm/$name` (79 §6.2, §6.5, §6.6): the live RFB session on a canvas. `scaleViewport: false`, `resizeSession: true`
 * (the window's size is the guest's geometry), `clipViewport: false`. View-only while a turn runs on the VM's engine;
 * **Take over** flips input on, steers the agent (`sandbox_takeover`) and paints the canvas border red; **Hand back**
 * reverses both. A dropped socket retries with backoff; a stopped VM shows **Start**. An unresolved hand-off for this
 * engine is a banner with Done · Cancel and arms Take over.
 */
export function VmDesktop({ name }: { name: string }) {
  const api = useApi()
  const list = useSandboxes()
  const act = useSandboxActions()
  const vm = list.data?.find((s) => s.name === name)
  const running = vm?.state.state === 'running'
  const access = useQuery({
    queryKey: ['sandboxes', 'desktop', name],
    queryFn: () => api.sandbox_desktop_access(name),
    enabled: running,
    retry: false,
    staleTime: Infinity,
  })
  const sessions = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions(), refetchInterval: 5000, enabled: !!vm?.engine })
  const agentBusy = !!vm?.engine && (sessions.data ?? []).some((r) => r.owner === vm.engine && r.running)
  const handoffs = useHandoffs(!!vm?.engine)
  const handoff = unresolved(handoffs.data, vm?.engine)
  const resolve = useAction(({ id, outcome }: { id: string; outcome: 'done' | 'cancelled' }) => api.handoff_resolve(id, outcome), { keys: [HANDOFFS_KEY] })
  const takeover = useAction((on: boolean) => api.sandbox_takeover(name, on), {
    onSuccess: (n, on) => {
      if (on && n > 0) toast.message(`Told ${n} running session${n === 1 ? '' : 's'} you have the desktop`)
    },
  })

  const box = useRef<HTMLDivElement>(null)
  const rfb = useRef<RFB | null>(null)
  const [conn, setConn] = useState<Conn>('connecting')
  const [attempt, setAttempt] = useState(0)
  const [reason, setReason] = useState<string | null>(null)
  const [geometry, setGeometry] = useState('')
  const [desktopName, setDesktopName] = useState('')
  const [taken, setTaken] = useState(false)
  const [fit, setFit] = useState(true)
  const viewOnly = agentBusy && !taken

  // One RFB per access grant and attempt; the cleanup disconnects and drops the canvas.
  useEffect(() => {
    const target = box.current
    const a = access.data
    if (!target || !a) return
    setConn(attempt === 0 ? 'connecting' : 'reconnecting')
    const r = new RFB(target, a.ws_url, { credentials: { password: a.password } })
    r.scaleViewport = false
    r.clipViewport = false
    r.resizeSession = fit
    r.viewOnly = viewOnly
    r.background = 'transparent'
    let timer: number | undefined
    let dead = false
    r.addEventListener('connect', () => {
      setConn('connected')
      setReason(null)
    })
    r.addEventListener('securityfailure', (e) => setReason(e.detail.reason ?? `security failure ${e.detail.status}`))
    r.addEventListener('desktopname', (e) => setDesktopName(e.detail.name))
    r.addEventListener('disconnect', () => {
      if (dead) return
      setConn('reconnecting')
      timer = window.setTimeout(() => setAttempt((n) => n + 1), RETRY_MS[Math.min(attempt, RETRY_MS.length - 1)])
    })
    rfb.current = r
    // The framebuffer size is the canvas's own size (unscaled): watch it for the geometry readout.
    const canvas = target.querySelector('canvas')
    const ro = new ResizeObserver(() => {
      if (canvas && canvas.width > 0) setGeometry(`${canvas.width}×${canvas.height}`)
    })
    if (canvas) ro.observe(canvas)
    return () => {
      dead = true
      clearTimeout(timer)
      ro.disconnect()
      rfb.current = null
      r.disconnect()
    }
    // `fit` and `viewOnly` are pushed below without a reconnect.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [access.data, attempt])
  useEffect(() => {
    if (rfb.current) rfb.current.viewOnly = viewOnly
  }, [viewOnly])
  useEffect(() => {
    if (rfb.current) rfb.current.resizeSession = fit
  }, [fit])
  // Another window (or the `sandbox_takeover` tool) flipped it.
  useEffect(() => on('Takeover', (e) => e.name === name && setTaken(e.on)), [name])
  // A hand-off arms Take over (79 §6.6) once per row.
  const armed = useRef<string | null>(null)
  useEffect(() => {
    if (!handoff || armed.current === handoff.id || taken) return
    armed.current = handoff.id
    setTaken(true)
    takeover.mutate(true)
  }, [handoff, taken, takeover])
  const flip = (on: boolean) => {
    setTaken(on)
    takeover.mutate(on)
  }

  if (list.isPending) return <Note>Loading…</Note>
  if (!vm) return <Note>No sandbox VM named {name}.</Note>
  const state = vm.state
  return (
    <div className="flex h-full min-h-0 flex-1 flex-col bg-black">
      <div className="flex h-9 shrink-0 items-center gap-2 bg-elevated px-2 text-xs text-fg-secondary">
        <span className="font-medium text-foreground">{vm.name}</span>
        <span>{stateLabel(state)}</span>
        {geometry && <span className="tabular-nums">{geometry}</span>}
        {desktopName && <span className="truncate text-fg-tertiary">{desktopName}</span>}
        <span className={cn('rounded-full px-1.5', conn === 'connected' ? 'bg-success-bg text-success' : conn === 'failed' ? 'bg-danger-bg text-danger' : 'bg-warning-bg text-warning')}>
          {conn === 'connected' ? (viewOnly ? 'watching' : taken ? 'taken over' : 'live') : conn}
        </span>
        <span className="ml-auto flex items-center gap-1">
          {running && (
            <Button size="xs" variant={taken ? 'destructive' : 'outline'} onClick={() => flip(!taken)} disabled={takeover.isPending}>
              {taken ? 'Hand back' : 'Take over'}
            </Button>
          )}
          {running && (
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button size="icon-xs" variant="ghost" aria-label="Send keys">
                  <Keyboard />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                {CHORDS.map(([label, send]) => (
                  <DropdownMenuItem key={label} disabled={viewOnly || conn !== 'connected'} onSelect={() => rfb.current && send(rfb.current)}>
                    {label}
                  </DropdownMenuItem>
                ))}
              </DropdownMenuContent>
            </DropdownMenu>
          )}
          {running && (
            <Button size="icon-xs" variant="ghost" aria-label={fit ? 'Actual size' : 'Fit to window'} title={fit ? 'Actual size' : 'Fit to window'} onClick={() => setFit((f) => !f)}>
              {fit ? <Minimize2 /> : <Maximize2 />}
            </Button>
          )}
          {running && (
            <Button size="icon-xs" variant="ghost" aria-label="Reconnect" title="Reconnect" onClick={() => setAttempt((n) => n + 1)}>
              <RotateCcw />
            </Button>
          )}
        </span>
      </div>
      {handoff && (
        <div className="flex flex-wrap items-center gap-2 bg-info-bg px-3 py-1.5 text-[13px] text-info">
          <span className="font-medium">{handoff.agent ?? handoff.session.slice(-6)} needs you:</span> {handoff.reason}
          {handoff.url && <span className="truncate font-mono text-xs text-fg-secondary">{handoff.url}</span>}
          <span className="ml-auto flex gap-1">
            <Button size="xs" disabled={resolve.isPending} onClick={() => resolve.mutate({ id: handoff.id, outcome: 'done' })}>
              Done
            </Button>
            <Button size="xs" variant="outline" disabled={resolve.isPending} onClick={() => resolve.mutate({ id: handoff.id, outcome: 'cancelled' })}>
              Cancel
            </Button>
          </span>
        </div>
      )}
      {!running ? (
        <Note>
          The VM is {stateLabel(state)}.
          {(state.state === 'ready' || state.state === 'stopped' || state.state === 'crashed') && (
            <Button size="sm" className="ml-3" disabled={act.start.isPending} onClick={() => act.start.mutate(name)}>
              <Play /> Start
            </Button>
          )}
          {state.state === 'paused' && (
            <Button size="sm" className="ml-3" disabled={act.resume.isPending} onClick={() => act.resume.mutate(name)}>
              <Play /> Resume
            </Button>
          )}
        </Note>
      ) : access.isError ? (
        <Note>No desktop access: {access.error.message}</Note>
      ) : (
        <div className={cn('relative min-h-0 flex-1', fit ? 'overflow-hidden' : 'overflow-auto')}>
          <div ref={box} className={cn('h-full w-full', taken && 'ring-4 ring-inset ring-danger', !fit && 'h-auto w-auto')} />
          {conn !== 'connected' && (
            <div className="pointer-events-none absolute inset-0 flex items-center justify-center text-sm text-fg-secondary">
              {conn === 'reconnecting' ? 'reconnecting…' : reason ?? 'connecting…'}
            </div>
          )}
        </div>
      )}
    </div>
  )
}

function Note({ children }: { children: React.ReactNode }) {
  return <div className="flex flex-1 items-center justify-center p-6 text-sm text-fg-secondary">{children}</div>
}
