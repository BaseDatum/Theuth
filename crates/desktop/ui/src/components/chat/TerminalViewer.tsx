import { useQuerySupport } from '@/rpc/capabilities'
import '@xterm/xterm/css/xterm.css'
import { FitAddon } from '@xterm/addon-fit'
import { Terminal as Xterm } from '@xterm/xterm'
import { useQuery } from '@tanstack/react-query'
import { AppWindow, Keyboard } from 'lucide-react'
import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog'
import { PtyIcon } from './Indicators'
import { Sheet, SheetClose, SheetContent, SheetTitle } from '@/components/ui/sheet'
import { useIsMobile } from '@/hooks/use-mobile'
import { isApple } from '@/lib/chord'
import { runsElsewhere } from '@/lib/front-door'
import { useEngineLabel } from '@/lib/owner'
import { touch } from '@/lib/platform'
import { canPopout, openPopout } from '@/lib/popout'
import { queryKeys, useApi } from '@/lib/rpc'
import { macKeyBytes } from '@/lib/terminal-keys'
import { swallowEngineQueries } from '@/lib/terminal-replies'
import { cn } from '@/lib/utils'
import type { Api } from '@/rpc/bindings/client'
import type { JobExit } from "@/rpc/bindings/method.agent.output"
import type { Key } from '@/rpc/bindings/method.agent.input'
import type { PtyChunk } from "@/rpc/bindings/method.agent.output"
import type { RunId as CellRunId } from "@/rpc/bindings/method.cells.output"
import type { RpcClient } from '@/rpc/client'
import type { TerminalSummary } from "@/rpc/bindings/method.agent.output"
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
/** A kernel PTY's fixed grid (171 §7.4): the cell viewer starts there and never resizes it. */
const CELL_ROWS = 30
const CELL_COLS = 100
/** 171F B12: a cell terminal opens at this many rows and grows with the rows its program used, up to `CELL_ROWS`. */
const CELL_MIN_ROWS = 12

/** The rows a cell terminal shows: the rows in use, at least `min`, at most the `grid`. */
export const liveRows = (used: number, min: number, grid: number) => Math.min(grid, Math.max(min, used))

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
/** The cell key row (171 §11.6): raw bytes through `cell_input`, arrows per the program's cursor-key mode. */
export const CELL_KEYS: [string, (app: boolean) => string][] = [
  ['↑', (app) => (app ? '\x1bOA' : '\x1b[A')],
  ['↓', (app) => (app ? '\x1bOB' : '\x1b[B')],
  ['⏎', () => '\r'],
  ['Esc', () => '\x1b'],
  ['Tab', () => '\t'],
  ['^C', () => '\x03'],
]

type JobRef = { session: string; job: string }

/** Where a `Terminal`'s bytes come from: an agent PTY job, or a cell run's kernel PTY (171 §11.1 output). */
export type TerminalSource = { kind: 'pty'; session: string; job: string } | { kind: 'cell'; run: CellRunId }

const bytes = (b64: string) => Uint8Array.from(atob(b64), (c) => c.charCodeAt(0))
/** Base64 of a string's UTF-8 bytes (`cell_input` takes base64). */
const utf8b64 = (s: string) => btoa(Array.from(new TextEncoder().encode(s), (b) => String.fromCharCode(b)).join(''))

/**
 * The wire between one source and an xterm (171 §7.4): bytes stream in (`subscribe_pty` / `cell_attach`), keystrokes
 * go out one call at a time in typing order (`pty_input` / `cell_input`), nothing is sent after `exit`. Hook-free so
 * the test drives it with fakes.
 */
export function attachSource(
  src: TerminalSource,
  term: Pick<Xterm, 'rows' | 'cols' | 'reset' | 'resize' | 'write' | 'onData' | 'attachCustomKeyEventHandler' | 'parser'>,
  api: Pick<Api, 'pty_input' | 'cell_input'>,
  client: Pick<RpcClient, 'stream'>,
  onExit: (e: JobExit) => void,
  /** 183 §4.3: the attach was refused `-32014 RunsElsewhere` — the live output stays on `owner`; nothing is attached or sent. */
  onElsewhere?: (owner: string) => void,
) {
  let ended = false
  let lane: Promise<unknown> = Promise.resolve()
  const call = src.kind === 'pty' ? (d: string) => api.pty_input(src.session, src.job, d, null) : (d: string) => api.cell_input(src.run, utf8b64(d))
  const send = (d: string) => {
    if (ended) return
    lane = lane.then(() => call(d).catch(() => undefined))
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
  const chunk = (c: PtyChunk) => {
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
        // Once: a reconnect replays the stream, exit included.
        if (ended) break
        ended = true
        onExit(c.exit)
        break
    }
  }
  const off = (
    src.kind === 'pty'
      ? client.stream<PtyChunk>('subscribe_pty', [src.session, src.job, term.rows, term.cols], 'unsubscribe_pty', chunk)
      : client.stream<PtyChunk>('cell_attach', [src.run], 'cell_detach', chunk)
  ).catch((e: unknown) => {
    const r = runsElsewhere(e)
    if (!r) throw e
    ended = true
    onElsewhere?.(r.owner)
    return async () => {}
  })
  return {
    send,
    ended: () => ended,
    dispose() {
      void off.then((f) => f())
      input.dispose()
    },
  }
}

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

export type TerminalProps = {
  label: string
  popped?: boolean
  /** Once, when the stream's `exit` chunk arrives. */
  onExit?: () => void
  /** 183 §4.3: the run's live output stays on another engine (`-32014`); its id. */
  onElsewhere?: (owner: string) => void
  className?: string
} & ((JobRef & { source?: undefined }) | { source: TerminalSource; session?: undefined; job?: undefined })

/**
 * One PTY in an xterm: the source streams the program's bytes (`subscribe_pty` / `cell_attach`), xterm emulates,
 * keystrokes go back as bytes (`pty_input` / `cell_input`). A pty source's grid follows the container (`pty_resize` →
 * SIGWINCH); a cell source's kernel PTY is fixed 100×30 (171 §7.4) and is never resized. Below it the mobile key
 * strip and the status line (`label` · size · exit). `popped`: this is the window's whole content — it owns the
 * PTY's size (leaves the `mark`, restores the default grid on close) and titles the window.
 */
export function Terminal({ session, job, label, popped = false, source, onExit, onElsewhere, className }: TerminalProps) {
  const api = useApi()
  const client = useConnection((s) => s.client)
  const mobile = useIsMobile()
  const host = useRef<HTMLDivElement>(null)
  const termRef = useRef<Xterm>(null)
  const sendRef = useRef<(d: string) => void>(null)
  const onExitRef = useRef(onExit)
  const onElsewhereRef = useRef(onElsewhere)
  const [elsewhere, setElsewhere] = useState<string | null>(null)
  const elsewhereLabel = useEngineLabel(elsewhere ?? '')
  const [exit, setExit] = useState<JobExit | null>(null)
  const [size, setSize] = useState('')
  // A cell source's height (171F B12): rows shown and the measured row height (px, 0 until xterm has rendered).
  const [rows, setRows] = useState(CELL_MIN_ROWS)
  const [rowH, setRowH] = useState(0)
  // The soft keyboard is up while xterm's textarea has focus (mobile toggle).
  const [keyboard, setKeyboard] = useState(false)
  // Stable by value, so an inline `source={{…}}` does not restart the stream every render.
  const run = source?.kind === 'cell' ? source.run : null
  const ptySession = source?.kind === 'pty' ? source.session : session
  const ptyJob = source?.kind === 'pty' ? source.job : job
  const src = useMemo<TerminalSource>(
    () => (run !== null ? { kind: 'cell', run } : { kind: 'pty', session: ptySession!, job: ptyJob! }),
    [run, ptySession, ptyJob],
  )
  useEffect(() => {
    onExitRef.current = onExit
    onElsewhereRef.current = onElsewhere
  }, [onExit, onElsewhere])

  useEffect(() => {
    const el = host.current
    if (!el || !client) return
    const ref = src.kind === 'pty' ? src : null
    setExit(null)
    setElsewhere(null)
    setRows(CELL_MIN_ROWS)
    const term = new Xterm({
      theme: { background: BG, foreground: FG, cursor: FG, selectionBackground: '#e8dcc855' },
      fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace',
      fontSize: 12,
      scrollback: 5000,
      cursorBlink: true,
      ...(ref ? {} : { rows: CELL_ROWS, cols: CELL_COLS }),
    })
    // Only a pty source's grid follows the box; a cell's is the kernel's fixed grid (the box scrolls or clips).
    const fit = ref ? new FitAddon() : null
    if (fit) term.loadAddon(fit)
    term.open(el)
    termRef.current = term
    fit?.fit()
    // A phone's keyboard covers half the terminal: tap (or the keyboard button) asks for it.
    if (!mobile) term.focus()
    const ta = term.textarea!
    const focused = () => setKeyboard(true)
    const blurred = () => setKeyboard(false)
    ta.addEventListener('focus', focused)
    ta.addEventListener('blur', blurred)
    // ponytail: xterm 6.0 lost touch scrolling (xtermjs/xterm.js#5489, fixed in 7); drag → scrollLines until then.
    let touchY: number | null = null
    const drag = (e: TouchEvent) => {
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
    el.addEventListener('touchstart', drag, { passive: true })
    el.addEventListener('touchmove', drag, { passive: false })
    const wire = attachSource(src, term, api, client, (e) => {
      setExit(e)
      onExitRef.current?.()
    }, (owner) => {
      setElsewhere(owner)
      onElsewhereRef.current?.(owner)
    })
    sendRef.current = wire.send
    // pty source only: size the PTY to the box, and give the default grid back on leaving.
    const resized = ref
      ? term.onResize(({ rows, cols }) => {
          setSize(`${cols}×${rows}`)
          if (!wire.ended()) void api.pty_resize(ref.session, ref.job, rows, cols).catch(() => undefined)
        })
      : null
    if (ref) setSize(`${term.cols}×${term.rows}`)
    const ro = fit ? new ResizeObserver(() => fit.fit()) : null
    ro?.observe(el)
    // Cell source: grow to the rows in use (a full-screen program or scrolled output takes the whole grid); the
    // kernel PTY itself is never resized.
    const grow = () => {
      const b = term.buffer.active
      const used = b.type === 'alternate' || b.baseY > 0 ? CELL_ROWS : b.cursorY + 1
      setRows((r) => Math.max(r, liveRows(used, CELL_MIN_ROWS, CELL_ROWS)))
      const screen = el.querySelector('.xterm-screen')?.getBoundingClientRect().height ?? 0
      if (screen > 0) setRowH(screen / term.rows)
    }
    const parsed = ref ? null : term.onWriteParsed(grow)
    if (!ref) grow()
    if (ref && popped) localStorage.setItem(mark(ref), '1')
    // Leaving: the popped window gives the grid back; the panel only when no popped window owns it.
    const leave = () => {
      if (!ref) return
      if (popped) localStorage.removeItem(mark(ref))
      if (!wire.ended() && (popped || !marked(ref))) void api.pty_resize(ref.session, ref.job, DEFAULT_ROWS, DEFAULT_COLS).catch(() => undefined)
    }
    window.addEventListener('pagehide', leave)
    return () => {
      wire.dispose()
      ro?.disconnect()
      parsed?.dispose()
      resized?.dispose()
      ta.removeEventListener('focus', focused)
      ta.removeEventListener('blur', blurred)
      el.removeEventListener('touchstart', drag)
      el.removeEventListener('touchmove', drag)
      window.removeEventListener('pagehide', leave)
      leave()
      termRef.current = null
      sendRef.current = null
      term.dispose()
      setKeyboard(false)
    }
  }, [src, client, api, mobile, popped])

  const status = [label, size, exit && exitText(exit), elsewhere !== null && `live output on ${elsewhereLabel}`].filter(Boolean).join(' · ')
  useEffect(() => {
    if (popped) document.title = status
  }, [popped, status])

  const keys = src.kind === 'pty' ? mobile : touch()
  return (
    <>
      <div
        // A cell box (a block child: WebKit grows only those by their own scrollbar) sizes to its rows plus its own
        // horizontal scrollbar (a 100-col grid is wider than a narrow box), so the bottom row — the cursor row, where a
        // prompt waits — is never under the bar. Past the caller's max-height it scrolls, and `flex-col-reverse` keeps
        // it at the bottom unless the user scrolls up.
        className={cn(
          'relative min-h-0 flex-1 rounded-md p-1',
          src.kind === 'cell' ? 'flex flex-col-reverse overflow-auto' : 'overflow-hidden',
          className,
        )}
        // `colorScheme`: the box's scrollbars match the soot canvas on a light app theme too.
        style={{ backgroundColor: BG, colorScheme: 'dark' }}
      >
        <div
          ref={host}
          className={src.kind === 'cell' ? 'w-max shrink-0 overflow-hidden' : 'size-full'}
          // The rows in use of the 30-row grid; ~15 px a 12 px row until xterm has measured one.
          style={src.kind === 'cell' ? { height: rows * (rowH || 15) } : undefined}
        />
      </div>
      {keys && !exit && (
        <div className="flex gap-1 overflow-x-auto" role="group" aria-label="Terminal keys">
          <Button
            variant={keyboard ? 'secondary' : 'outline'}
            size="sm"
            className={src.kind === 'pty' ? 'h-11 min-w-11' : 'h-12 min-w-12'}
            aria-pressed={keyboard}
            aria-label={keyboard ? 'Hide keyboard' : 'Show keyboard'}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => (keyboard ? termRef.current?.blur() : termRef.current?.focus())}
          >
            <Keyboard />
          </Button>
          {src.kind === 'pty'
            ? TOOLBAR.map(([text, k]) => (
                <Button
                  key={text}
                  variant="outline"
                  size="sm"
                  className="h-11 min-w-11 font-mono"
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => void api.pty_input(src.session, src.job, null, [k]).catch(() => undefined)}
                >
                  {text}
                </Button>
              ))
            : CELL_KEYS.map(([text, seq]) => (
                <Button
                  key={text}
                  variant="outline"
                  size="sm"
                  className="h-12 min-w-12 font-mono"
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => sendRef.current?.(seq(termRef.current?.modes.applicationCursorKeysMode ?? false))}
                >
                  {text}
                </Button>
              ))}
        </div>
      )}
      {/* A cell's terminal lives in a note's editor: no live region there (the cell announces its run, CELLHIST). */}
      <p aria-live={src.kind === 'pty' ? 'polite' : undefined} className={cn('truncate text-xs text-fg-secondary', exit && 'text-warning')} title={status}>
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
  const canQuery1 = useQuerySupport(["list_terminals"])
  const api = useApi()
  const live = useSessionField(session ?? '', 'terminal')
  const { data } = useQuery({
    queryKey: queryKeys.terminals(session ?? ''),
    queryFn: () => api.list_terminals(session ?? '', true),
    enabled: canQuery1 && (session !== null && !live),
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
