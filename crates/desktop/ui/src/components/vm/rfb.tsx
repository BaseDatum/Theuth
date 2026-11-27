// The noVNC session shared by the sandbox VM desktop (`VmDesktop`) and a cloud node's watchable desktop
// (`cloud/NodeDesktop`): one RFB on a canvas per access grant and attempt, reconnect with backoff, and the send-keys menu.
// Who may type (view-only) and what Take over means beyond that stay with each caller.
import type RFB from '@novnc/novnc'
import { Keyboard } from 'lucide-react'
import { type RefObject, useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'

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

export type Conn = 'connecting' | 'connected' | 'reconnecting' | 'failed'
const RETRY_MS = [1000, 2000, 4000, 8000, 15000]

export type RfbAccess = { ws_url: string; password: string }

export type Rfb = {
  /** The element the canvas mounts in. */
  box: RefObject<HTMLDivElement | null>
  rfb: RefObject<RFB | null>
  conn: Conn
  reason: string | null
  /** `1280×800`, from the framebuffer. */
  geometry: string
  desktopName: string
  reconnect: () => void
}

/**
 * One RFB per access grant and attempt; the cleanup disconnects and drops the canvas. `scaleViewport: false`,
 * `resizeSession: fit` (the box's size is the guest's geometry), `clipViewport: false`. `viewOnly` and `fit` are pushed
 * without a reconnect. A dropped socket retries with backoff.
 */
export function useRfb(access: RfbAccess | null | undefined, viewOnly: boolean, fit: boolean): Rfb {
  const box = useRef<HTMLDivElement>(null)
  const rfb = useRef<RFB | null>(null)
  const [conn, setConn] = useState<Conn>('connecting')
  const [attempt, setAttempt] = useState(0)
  const [reason, setReason] = useState<string | null>(null)
  const [geometry, setGeometry] = useState('')
  const [desktopName, setDesktopName] = useState('')
  // The latest `viewOnly` / `fit`, for a session that connects after noVNC loads.
  const latest = useRef({ viewOnly, fit })
  latest.current = { viewOnly, fit }

  useEffect(() => {
    const target = box.current
    if (!target || !access) return
    setConn(attempt === 0 ? 'connecting' : 'reconnecting')
    let r: RFB | null = null
    let ro: ResizeObserver | null = null
    let timer: number | undefined
    let dead = false
    // noVNC is loaded on first use: it reads `window` at import, and most screens that can open a desktop never do.
    void import('@novnc/novnc').then(({ default: Rfb }) => {
      if (dead) return
      const s = new Rfb(target, access.ws_url, { credentials: { password: access.password } })
      r = s
      s.scaleViewport = false
      s.clipViewport = false
      s.resizeSession = latest.current.fit
      s.viewOnly = latest.current.viewOnly
      s.background = 'transparent'
      s.addEventListener('connect', () => {
        setConn('connected')
        setReason(null)
      })
      s.addEventListener('securityfailure', (e) => setReason(e.detail.reason ?? `security failure ${e.detail.status}`))
      s.addEventListener('desktopname', (e) => setDesktopName(e.detail.name))
      s.addEventListener('disconnect', () => {
        if (dead) return
        setConn('reconnecting')
        timer = window.setTimeout(() => setAttempt((n) => n + 1), RETRY_MS[Math.min(attempt, RETRY_MS.length - 1)])
      })
      rfb.current = s
      // The framebuffer size is the canvas's own size (unscaled): watch it for the geometry readout.
      const canvas = target.querySelector('canvas')
      ro = new ResizeObserver(() => {
        if (canvas && canvas.width > 0) setGeometry(`${canvas.width}×${canvas.height}`)
      })
      if (canvas) ro.observe(canvas)
    })
    return () => {
      dead = true
      clearTimeout(timer)
      ro?.disconnect()
      rfb.current = null
      r?.disconnect()
    }
  }, [access, attempt])
  useEffect(() => {
    if (rfb.current) rfb.current.viewOnly = viewOnly
  }, [viewOnly])
  useEffect(() => {
    if (rfb.current) rfb.current.resizeSession = fit
  }, [fit])

  return { box, rfb, conn, reason, geometry, desktopName, reconnect: () => setAttempt((n) => n + 1) }
}

/** The keyboard menu: chords a browser cannot pass through (Ctrl-Alt-Del, Alt-Tab, …). */
export function SendKeysMenu({ rfb, disabled }: { rfb: RefObject<RFB | null>; disabled: boolean }) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button size="icon-xs" variant="ghost" aria-label="Send keys">
          <Keyboard />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        {CHORDS.map(([label, send]) => (
          <DropdownMenuItem key={label} disabled={disabled} onSelect={() => rfb.current && send(rfb.current)}>
            {label}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
