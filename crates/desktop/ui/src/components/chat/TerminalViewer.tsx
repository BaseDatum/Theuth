import '@xterm/xterm/css/xterm.css'
import { FitAddon } from '@xterm/addon-fit'
import { Terminal as Xterm } from '@xterm/xterm'
import { useQuery } from '@tanstack/react-query'
import { AppWindow, Keyboard } from 'lucide-react'
import { useEffect, useRef, useState, useSyncExternalStore } from 'react'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog'
import { PtyIcon } from './Indicators'
import { Sheet, SheetClose, SheetContent, SheetTitle } from '@/components/ui/sheet'
import { useIsMobile } from '@/hooks/use-mobile'
import { isApple } from '@/lib/chord'
import { canPopout, openPopout } from '@/lib/popout'
import { queryKeys, useApi } from '@/lib/rpc'
import { macKeyBytes } from '@/lib/terminal-keys'
import { swallowEngineQueries } from '@/lib/terminal-replies'
import { cn } from '@/lib/utils'
import type { Api } from '@/rpc/api'
import type { JobExit } from '@/rpc/bindings/JobExit'
import type { Key } from '@/rpc/bindings/Key'
import type { PtyChunk } from '@/rpc/bindings/PtyChunk'
import type { CellRunId } from '@/rpc/bindings/CellRunId'
import type { TerminalSummary } from '@/rpc/bindings/TerminalSummary'
import { useConnection } from '@/store/connection'
import { useSessionField } from '@/store/sessions'
import { MenuRow } from './HeaderMenu'

// Fixed soot canvas so the 16-colour palette reads on either app theme.
const BG = '#12100e'
const FG = '#e8dcc8'
/**
 * The grid a job starts with (pty.rs `ROWS`/`COLS`); restored when the viewer leaves so the model's `screen` is not left phone-sized.
 * Two viewers on one PTY would fight over it, so exactly one sizes it: while a job's popped window is open (its
 * `mark` in the shared localStorage) the panel shows a stub instead of an xterm and does not restore the grid when
 * it leaves; the popped window sizes the PTY to itself and restores the default on close (`pagehide`).
 */
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

const labelOf = (t: TerminalSummary | undefined, job: JobRef) => (t ? `${t.name ?? t.command} · ${t.state}` : job.job)

/** `list_terminals(session, descendants)`, polled while mounted: the selector's rows and each job's name/state. */
const terminalsQuery = (api: Api, session: string) => ({
  queryKey: queryKeys.terminals(session),
  queryFn: () => api.list_terminals(session, true),
  staleTime: 0,
  refetchInterval: 5000,
  refetchIntervalInBackground: false,
})

// The mark a popped window leaves for the panel in the other window (`storage` events cross windows, like `composer-drafts`).
const mark = (j: JobRef) => `pty-popout:${j.session}/${j.job}`
const marked = (j: JobRef) => localStorage.getItem(mark(j)) !== null
const onStorage = (cb: () => void) => {
  window.addEventListener('storage', cb)
  return () => window.removeEventListener('storage', cb)
}
/** Whether `job` is showing in its own window right now (the panel then defers to it). */
const usePoppedElsewhere = (job: JobRef | null) => useSyncExternalStore(onStorage, () => job !== null && marked(job))

/** "Open in new window" for one terminal; nothing where no second window exists. */
export function PopoutTerminalButton({ session, job, className }: JobRef & { className?: string }) {
  if (!canPopout()) return null
  return (
    <Button
      variant="ghost"
      className={cn('size-9 shrink-0 rounded-lg text-fg-secondary pointer-coarse:size-11', className)}
      aria-label="Open in new window"
      onClick={() => void openPopout({ kind: 'pty', session, job })}
    >
      <AppWindow className="size-5" />
    </Button>
  )
}

/**
 * One PTY job in an xterm: the daemon streams the program's bytes (`subscribe_pty`), xterm emulates, keystrokes go
 * back as bytes (`pty_input`), and the grid follows the container (`pty_resize` → SIGWINCH). Below it the mobile key
 * strip and the status line (`label` · size · exit). `popped`: this is the window's whole content — it owns the
 * PTY's size (leaves the `mark`, restores the default grid on close) and titles the window.
 */
export function Terminal({
  session,
  job,
  label,
  popped = false,
}: JobRef & {
  label: string
  popped?: boolean
  // 171:U1 a cell run's bytes come from `cell_attach` instead of `subscribe_pty` (171 §11.1 output); unused until U1.
  source?: { kind: 'cell'; run: CellRunId }
}) {
  const api = useApi()
  const client = useConnection((s) => s.client)
  const mobile = useIsMobile()
  const host = useRef<HTMLDivElement>(null)
  const termRef = useRef<Xterm>(null)
  const [exit, setExit] = useState<JobExit | null>(null)
  const [size, setSize] = useState('')
  // The soft keyboard is up while xterm's textarea has focus (mobile toggle).
  const [keyboard, setKeyboard] = useState(false)

  useEffect(() => {
    const el = host.current
    if (!el || !client) return
    const ref = { session, job }
    setExit(null)
    const term = new Xterm({
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
    // A phone's keyboard covers half the terminal: tap (or the keyboard button) asks for it.
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
    const send = (d: string) => {
      if (ended) return
      lane = lane.then(() => api.pty_input(session, job, d, null).catch(() => undefined))
    }
    const input = term.onData(send)
    // 167 D1: macOS editing keys as Terminal.app sends them, through the same lane.
    const mac = isApple()
    term.attachCustomKeyEventHandler((e) => {
      const d = macKeyBytes(e, mac)
      if (d === null) return true
      e.preventDefault()
      send(d)
      return false
    })
    // 167 D2: the engine is the one responder for the queries it answers; this xterm keeps the rest.
    swallowEngineQueries(term.parser)
    const resized = term.onResize(({ rows, cols }) => {
      setSize(`${cols}×${rows}`)
      if (!ended) void api.pty_resize(session, job, rows, cols).catch(() => undefined)
    })
    setSize(`${term.cols}×${term.rows}`)
    const ro = new ResizeObserver(() => fit.fit())
    ro.observe(el)
    if (popped) localStorage.setItem(mark(ref), '1')
    // Leaving: the popped window gives the grid back; the panel only when no popped window owns it.
    const leave = () => {
      if (popped) localStorage.removeItem(mark(ref))
      if (!ended && (popped || !marked(ref))) void api.pty_resize(session, job, DEFAULT_ROWS, DEFAULT_COLS).catch(() => undefined)
    }
    window.addEventListener('pagehide', leave)
    const off = client.stream<PtyChunk>('subscribe_pty', [session, job, term.rows, term.cols], 'unsubscribe_pty', (c) => {
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
      window.removeEventListener('pagehide', leave)
      leave()
      termRef.current = null
      term.dispose()
      setKeyboard(false)
    }
  }, [session, job, client, api, mobile, popped])

  const status = [label, size, exit && exitText(exit)].filter(Boolean).join(' · ')
  useEffect(() => {
    if (popped) document.title = status
  }, [popped, status])

  return (
    <>
      <div className="relative min-h-0 flex-1 overflow-hidden rounded-md p-1" style={{ backgroundColor: BG }}>
        <div ref={host} className="size-full" />
      </div>
      {mobile && !exit && (
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
          {TOOLBAR.map(([text, k]) => (
            <Button
              key={text}
              variant="outline"
              size="sm"
              className="h-11 min-w-11 font-mono"
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => void api.pty_input(session, job, null, [k]).catch(() => undefined)}
            >
              {text}
            </Button>
          ))}
        </div>
      )}
      <p aria-live="polite" className={cn('truncate text-xs text-fg-secondary', exit && 'text-warning')} title={status}>
        {status}
      </p>
    </>
  )
}

/** `/pty/$session/$job` in its own window: the terminal alone, full height. Stays up after the job exits (the final screen is the point). */
export function PoppedTerminal({ session, job }: JobRef) {
  const api = useApi()
  const { data: terminals = [] } = useQuery(terminalsQuery(api, session))
  const current = terminals.find((t) => t.id === job && t.session === session)
  return (
    <div className="flex h-full min-h-0 flex-1 flex-col gap-2 p-2">
      <Terminal session={session} job={job} label={labelOf(current, { session, job })} popped />
    </div>
  )
}

function Body({ session }: { session: string }) {
  const api = useApi()
  const [job, setJob] = useState<JobRef | null>(null)
  const { data: terminals = [] } = useQuery(terminalsQuery(api, session))
  const first = terminals[0]
  useEffect(() => {
    if (!job && first) setJob({ session: first.session, job: first.id })
  }, [job, first])
  const elsewhere = usePoppedElsewhere(job)
  const current = terminals.find((t) => t.id === job?.job && t.session === job?.session)

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2 p-2">
      <div className="flex gap-2">
        <select
          aria-label="Terminal"
          className="h-9 min-w-0 flex-1 rounded-md border border-border-subtle bg-background px-2 text-sm pointer-coarse:h-11"
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
        {job && <PopoutTerminalButton {...job} />}
      </div>
      {!job ? (
        <>
          <div className="flex min-h-0 flex-1 rounded-md p-4 text-sm" style={{ backgroundColor: BG, color: FG }}>
            Pick a terminal.
          </div>
          <p className="truncate text-xs text-fg-secondary">No terminals</p>
        </>
      ) : elsewhere ? (
        <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 rounded-md p-4 text-sm" style={{ backgroundColor: BG, color: FG }}>
          <p>{labelOf(current, job)} · opened in its own window</p>
          <Button variant="outline" size="sm" onClick={() => void openPopout({ kind: 'pty', ...job })}>
            Focus window
          </Button>
        </div>
      ) : (
        <Terminal key={`${job.session}/${job.job}`} session={job.session} job={job.job} label={labelOf(current, job)} />
      )}
    </div>
  )
}

/**
 * The session's PTY jobs (51 §7.3, 55 §4) in a real terminal. Dialog on desktop, full-height sheet on mobile. Streams
 * only while open. `/terminals` reaches it through the `theuth:terminals` window event.
 */
export function TerminalViewer({ session, open, onOpenChange }: { session: string; open: boolean; onOpenChange(o: boolean): void }) {
  const mobile = useIsMobile()
  useEffect(() => {
    const h = (e: Event) => {
      if ((e as CustomEvent<{ session?: string }>).detail?.session === session) onOpenChange(true)
    }
    window.addEventListener('theuth:terminals', h)
    return () => window.removeEventListener('theuth:terminals', h)
  }, [session, onOpenChange])
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
 * The session (or its subtree) has a PTY: a live `JobStarted.pty` or a retained terminal found on load, the same way
 * the globe follows the first `browser_*` call.
 */
export function useHasTerminals(session: string | null): boolean {
  const api = useApi()
  const live = useSessionField(session ?? '', 'terminal')
  const { data } = useQuery({
    queryKey: queryKeys.terminals(session ?? ''),
    queryFn: () => api.list_terminals(session ?? '', true),
    enabled: session !== null && !live,
    staleTime: Infinity,
  })
  return live || (data?.length ?? 0) > 0
}

/**
 * Header icon (or compact menu `row`) opening `TerminalViewer`, which the header owns; shown once `useHasTerminals`.
 * ⌥-click opens the session's first terminal in its own window instead (the POPOUT ⌥ convention).
 */
export function TerminalButton({ session, row, onOpen }: { session: string; row?: boolean; onOpen(): void }) {
  const api = useApi()
  const alt = useRef(false)
  // A retained-only terminal (found by `list_terminals`) is not live: the glyph animates on `terminal` alone.
  const live = !!useSessionField(session, 'terminal')
  if (!useHasTerminals(session)) return null
  const open = (popout: boolean) => {
    if (!popout || !canPopout()) return onOpen()
    void api
      .list_terminals(session, true)
      .then((ts) => (ts[0] ? openPopout({ kind: 'pty', session: ts[0].session, job: ts[0].id }) : onOpen()))
      .catch(onOpen)
  }
  if (row) return <MenuRow icon={<PtyIcon live={live} className="size-4 shrink-0 text-fg-secondary" />} label="Terminals" onClick={(e) => (alt.current = e.altKey)} onSelect={() => open(alt.current)} />
  return (
    <Button variant="ghost" className="size-9 rounded-lg text-fg-secondary" aria-label="Terminals" onClick={(e) => open(e.altKey)}>
      <PtyIcon live={live} className="size-5" />
    </Button>
  )
}
