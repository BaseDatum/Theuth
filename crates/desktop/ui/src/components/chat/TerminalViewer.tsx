import '@xterm/xterm/css/xterm.css'
import { FitAddon } from '@xterm/addon-fit'
import { Terminal } from '@xterm/xterm'
import { useQuery } from '@tanstack/react-query'
import { Keyboard, SquareTerminal } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog'
import { Sheet, SheetClose, SheetContent, SheetTitle } from '@/components/ui/sheet'
import { useIsMobile } from '@/hooks/use-mobile'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { JobExit } from '@/rpc/bindings/JobExit'
import type { Key } from '@/rpc/bindings/Key'
import type { PtyChunk } from '@/rpc/bindings/PtyChunk'
import { useConnection } from '@/store/connection'
import { useSessionField } from '@/store/sessions'

// Fixed soot canvas so the 16-colour palette reads on either app theme.
const BG = '#12100e'
const FG = '#e8dcc8'
/** The grid a job starts with (pty.rs `ROWS`/`COLS`); restored when the viewer leaves so the model's `screen` is not left phone-sized. */
const DEFAULT_ROWS = 40
const DEFAULT_COLS = 120

/** Mobile toolbar keys (55 §4.5): what a soft keyboard lacks. The daemon encodes them for the program's modes. */
const TOOLBAR: [string, Key][] = [
  ['Esc', 'escape'],
  ['Tab', 'tab'],
  ['↑', 'up'],
  ['↓', 'down'],
  ['←', 'left'],
  ['→', 'right'],
  ['^C', 'ctrl_c'],
]

type JobRef = { session: string; job: string }

const bytes = (b64: string) => Uint8Array.from(atob(b64), (c) => c.charCodeAt(0))

const exitText = (e: JobExit) => (typeof e === 'string' ? e : `exit ${e.exited}`)

function Body({ session }: { session: string }) {
  const api = useApi()
  const client = useConnection((s) => s.client)
  const mobile = useIsMobile()
  const host = useRef<HTMLDivElement>(null)
  const termRef = useRef<Terminal>(null)
  const [job, setJob] = useState<JobRef | null>(null)
  const [exit, setExit] = useState<JobExit | null>(null)
  const [size, setSize] = useState('')
  // The soft keyboard is up while xterm's textarea has focus (mobile toggle).
  const [keyboard, setKeyboard] = useState(false)
  const { data: terminals = [] } = useQuery({
    queryKey: queryKeys.terminals(session),
    queryFn: () => api.list_terminals(session, true),
    staleTime: 0,
    refetchInterval: 5000,
    refetchIntervalInBackground: false,
  })
  const first = terminals[0]
  useEffect(() => {
    if (!job && first) setJob({ session: first.session, job: first.id })
  }, [job, first])

  // One xterm per selected job: the daemon streams the program's bytes (`subscribe_pty`), xterm emulates,
  // keystrokes go back as bytes (`pty_input`), and the grid follows the container (`pty_resize` → SIGWINCH).
  useEffect(() => {
    const el = host.current
    if (!el || !job || !client) return
    setExit(null)
    const term = new Terminal({
      theme: { background: BG, foreground: FG, cursor: FG, selectionBackground: '#e8dcc855' },
      fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace',
      fontSize: 12,
      scrollback: 5000,
      cursorBlink: true,
    })
    const fit = new FitAddon()
    term.loadAddon(fit)
    term.open(el)
    termRef.current = term
    fit.fit()
    // A phone's keyboard covers half the terminal: tap (or the ⌨ button) asks for it.
    if (!mobile) term.focus()
    const ta = term.textarea!
    const focused = () => setKeyboard(true)
    const blurred = () => setKeyboard(false)
    ta.addEventListener('focus', focused)
    ta.addEventListener('blur', blurred)
    // ponytail: xterm 6.0 lost touch scrolling (xtermjs/xterm.js#5489, fixed in 7); drag → scrollLines until then.
    let touchY: number | null = null
    const touch = (e: TouchEvent) => {
      const t = e.touches[0]
      if (!t || e.touches.length !== 1) return
      const y = t.clientY
      if (e.type === 'touchstart' || touchY === null) {
        touchY = y
        return
      }
      const lines = Math.trunc((touchY - y) / (el.clientHeight / term.rows))
      if (lines !== 0) {
        term.scrollLines(lines)
        touchY = y
        e.preventDefault()
      }
    }
    el.addEventListener('touchstart', touch, { passive: true })
    el.addEventListener('touchmove', touch, { passive: false })
    let ended = false
    // One call in flight at a time: keystrokes reach the process in typing order.
    let lane: Promise<unknown> = Promise.resolve()
    const input = term.onData((d) => {
      if (ended) return
      lane = lane.then(() => api.pty_input(job.session, job.job, d, null).catch(() => undefined))
    })
    const resized = term.onResize(({ rows, cols }) => {
      setSize(`${cols}×${rows}`)
      if (!ended) void api.pty_resize(job.session, job.job, rows, cols).catch(() => undefined)
    })
    setSize(`${term.cols}×${term.rows}`)
    const ro = new ResizeObserver(() => fit.fit())
    ro.observe(el)
    // ponytail: last resize wins across viewers; a second open viewer sees the other's width until it refits.
    const off = client.stream<PtyChunk>('subscribe_pty', [job.session, job.job, term.rows, term.cols], 'unsubscribe_pty', (c) => {
      switch (c.type) {
        case 'reset':
          term.reset()
          if (term.rows !== c.rows || term.cols !== c.cols) term.resize(c.cols, c.rows)
          term.write(bytes(c.data))
          break
        case 'data':
          term.write(bytes(c.data))
          break
        case 'exit':
          ended = true
          setExit(c.exit)
          break
      }
    })
    return () => {
      void off.then((f) => f())
      ro.disconnect()
      input.dispose()
      resized.dispose()
      ta.removeEventListener('focus', focused)
      ta.removeEventListener('blur', blurred)
      el.removeEventListener('touchstart', touch)
      el.removeEventListener('touchmove', touch)
      if (!ended) void api.pty_resize(job.session, job.job, DEFAULT_ROWS, DEFAULT_COLS).catch(() => undefined)
      termRef.current = null
      term.dispose()
      setKeyboard(false)
    }
  }, [job, client, api, mobile])

  const current = terminals.find((t) => t.id === job?.job && t.session === job?.session)
  const status = [
    current ? `${current.name ?? current.command} · ${current.state}` : job ? job.job : 'No terminals',
    size,
    exit && exitText(exit),
  ]
    .filter(Boolean)
    .join(' · ')

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2 p-2">
      <select
        aria-label="Terminal"
        className="h-9 w-full rounded-md border border-border-subtle bg-background px-2 text-sm pointer-coarse:h-11"
        value={job ? `${job.session}/${job.job}` : ''}
        onChange={(e) => {
          const t = terminals.find((x) => `${x.session}/${x.id}` === e.target.value)
          if (t) setJob({ session: t.session, job: t.id })
        }}
      >
        {terminals.length === 0 && <option value="">No terminals in this session</option>}
        {terminals.map((t) => (
          <option key={`${t.session}/${t.id}`} value={`${t.session}/${t.id}`}>
            {t.name ?? t.command}
            {t.session !== session ? ' (subagent)' : ''} · {t.state}
          </option>
        ))}
      </select>
      <div className="relative min-h-0 flex-1 overflow-hidden rounded-md p-1" style={{ backgroundColor: BG }}>
        <div ref={host} className="size-full" />
        {!job && (
          <p className="absolute inset-0 p-4 text-sm" style={{ color: FG }}>
            Pick a terminal.
          </p>
        )}
      </div>
      {mobile && job && !exit && (
        <div className="flex gap-1 overflow-x-auto" role="group" aria-label="Terminal keys">
          <Button
            variant={keyboard ? 'secondary' : 'outline'}
            size="sm"
            className="h-11 min-w-11"
            aria-pressed={keyboard}
            aria-label={keyboard ? 'Hide keyboard' : 'Show keyboard'}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => (keyboard ? termRef.current?.blur() : termRef.current?.focus())}
          >
            <Keyboard />
          </Button>
          {TOOLBAR.map(([label, k]) => (
            <Button
              key={label}
              variant="outline"
              size="sm"
              className="h-11 min-w-11 font-mono"
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => void api.pty_input(job.session, job.job, null, [k]).catch(() => undefined)}
            >
              {label}
            </Button>
          ))}
        </div>
      )}
      <p aria-live="polite" className={cn('truncate text-xs text-fg-secondary', exit && 'text-warning')} title={status}>
        {status}
      </p>
    </div>
  )
}

/** The session's PTY jobs (51 §7.3, 55 §4) in a real terminal. Dialog on desktop, full-height sheet on mobile. Streams only while open. */
export function TerminalViewer({ session, open, onOpenChange }: { session: string; open: boolean; onOpenChange(o: boolean): void }) {
  const mobile = useIsMobile()
  if (mobile)
    return (
      <Sheet open={open} onOpenChange={onOpenChange}>
        <SheetContent side="bottom" showCloseButton={false} className="top-0 h-band gap-0 pt-[var(--safe-top)]">
          <div className="flex h-12 shrink-0 items-center border-b border-border-subtle px-4">
            <SheetTitle className="text-base">Terminals</SheetTitle>
            <SheetClose className="ml-auto flex size-11 items-center justify-center rounded-full text-fg-secondary active:bg-layer-3">
              <span aria-hidden>✕</span>
              <span className="sr-only">Close</span>
            </SheetClose>
          </div>
          {open && <Body session={session} />}
        </SheetContent>
      </Sheet>
    )
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex h-[85vh] max-w-[95vw] flex-col gap-0 p-0 pt-10 sm:max-w-[95vw]" onOpenAutoFocus={(e) => e.preventDefault()}>
        <DialogTitle className="sr-only">Terminals</DialogTitle>
        {open && <Body session={session} />}
      </DialogContent>
    </Dialog>
  )
}

/**
 * Header icon, shown once the session (or its subtree) has a PTY: a live `JobStarted.pty` or a retained terminal
 * found on load, the same way the globe follows the first `browser_*` call. `/terminals` reaches the viewer through
 * the `theuth:terminals` window event even before then.
 */
export function TerminalButton({ session }: { session: string }) {
  const api = useApi()
  const [open, setOpen] = useState(false)
  const live = useSessionField(session, 'terminal')
  const { data } = useQuery({
    queryKey: queryKeys.terminals(session),
    queryFn: () => api.list_terminals(session, true),
    enabled: !live,
    staleTime: Infinity,
  })
  useEffect(() => {
    const h = (e: Event) => {
      if ((e as CustomEvent<{ session?: string }>).detail?.session === session) setOpen(true)
    }
    window.addEventListener('theuth:terminals', h)
    return () => window.removeEventListener('theuth:terminals', h)
  }, [session])
  return (
    <>
      {(live || (data?.length ?? 0) > 0) && (
        <Button variant="ghost" className="size-9 rounded-lg text-fg-secondary" aria-label="Terminals" onClick={() => setOpen(true)}>
          <SquareTerminal className="size-5" />
        </Button>
      )}
      <TerminalViewer session={session} open={open} onOpenChange={setOpen} />
    </>
  )
}
