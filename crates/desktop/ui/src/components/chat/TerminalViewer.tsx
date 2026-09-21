import { useQuery, useQueryClient } from '@tanstack/react-query'
import { ChevronDown, ChevronUp, ChevronsDown, ChevronsUp, Keyboard, SquareTerminal } from 'lucide-react'
import { type CSSProperties, Fragment, useEffect, useReducer, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog'
import { Sheet, SheetClose, SheetContent, SheetTitle } from '@/components/ui/sheet'
import { useIsMobile } from '@/hooks/use-mobile'
import { queryKeys, useApi } from '@/lib/rpc'
import { EMPTY, type Nav, css, keyToInput, reduce } from '@/lib/terminal'
import { cn } from '@/lib/utils'
import type { Cell } from '@/rpc/bindings/Cell'
import type { Key } from '@/rpc/bindings/Key'
import type { PtyPage } from '@/rpc/bindings/PtyPage'
import type { Style } from '@/rpc/bindings/Style'
import type { StyledRun } from '@/rpc/bindings/StyledRun'
import { useScope } from '@/store/harness-scope'
import { useSessionField } from '@/store/sessions'

// Fixed soot canvas so the 16-colour palette reads on either app theme.
const BG = '#12100e'
const FG = '#e8dcc8'

const NAV_KEYS: Record<string, Nav> = { ArrowUp: 'up', ArrowDown: 'down', PageUp: 'pageUp', PageDown: 'pageDown', End: 'live' }

function inline(s: Style): CSSProperties | undefined {
  let fg = css(s.fg)
  let bg = css(s.bg)
  if (s.inverse) [fg, bg] = [bg ?? BG, fg ?? FG]
  if (!fg && !bg && !s.bold && !s.dim && !s.italic && !s.underline) return undefined
  return {
    color: fg,
    backgroundColor: bg,
    fontWeight: s.bold ? 'bold' : undefined,
    opacity: s.dim ? 0.6 : undefined,
    fontStyle: s.italic ? 'italic' : undefined,
    textDecoration: s.underline ? 'underline' : undefined,
  }
}

/** Wide continuations carry no text of their own. */
const text = (cells: Cell[]) => cells.reduce((s, c) => (c.continuation ? s : s + c.text), '')

function Row({ runs, cursor }: { runs: StyledRun[]; cursor: number | null }) {
  let col = 0
  return runs.map((run, i) => {
    const start = col
    col += run.cells.length
    const style = inline(run.style)
    if (cursor === null || cursor < start || cursor >= col)
      return (
        <span key={i} style={style}>
          {text(run.cells)}
        </span>
      )
    const k = cursor - start
    return (
      <span key={i} style={style}>
        {text(run.cells.slice(0, k))}
        <span style={{ color: BG, backgroundColor: FG }}>{run.cells[k]?.text || ' '}</span>
        {text(run.cells.slice(k + 1))}
      </span>
    )
  })
}

/** One page as a fixed-cell grid: a `<pre>` of styled runs, the cursor cell inverted, wide continuations skipped. `fontSize` shrinks the grid to the width it is given. */
export function Screen({ page, fontSize }: { page: PtyPage; fontSize?: number }) {
  return (
    <pre
      className="m-0 min-w-max select-text p-2 font-mono text-xs leading-[1.3] whitespace-pre"
      style={{ backgroundColor: BG, color: FG, minHeight: `${page.lines.length}lh`, fontSize }}
    >
      {page.lines.map((runs, r) => (
        <Fragment key={r}>
          <Row runs={runs} cursor={page.cursor && page.cursor[0] === r ? page.cursor[1] : null} />
          {'\n'}
        </Fragment>
      ))}
    </pre>
  )
}

/** Font size at which `cols` monospace cells fit `width` px, capped at the default 12px. */
const fit = (width: number, cols: number) => Math.min(12, Math.max(6, (width - 16) / (cols * 0.602)))

/** Mobile toolbar keys (55 §4.5): what a soft keyboard lacks. */
const TOOLBAR: [string, Key][] = [
  ['Esc', 'escape'],
  ['Tab', 'tab'],
  ['↑', 'up'],
  ['↓', 'down'],
  ['←', 'left'],
  ['→', 'right'],
  ['^C', { ctrl: 'c' }],
]

function Body({ session, open }: { session: string; open: boolean }) {
  const api = useApi()
  const queryClient = useQueryClient()
  const mobile = useIsMobile()
  const harness = useScope().id
  const [st, dispatch] = useReducer(reduce, EMPTY)
  // 55 §4: keys go to the process while the hidden textarea has focus.
  const [typing, setTyping] = useState(false)
  const input = useRef<HTMLTextAreaElement>(null)
  const screen = useRef<HTMLDivElement>(null)
  const [fontSize, setFontSize] = useState<number>()
  const { data: terminals = [] } = useQuery({
    queryKey: queryKeys.terminals(session),
    queryFn: () => api.list_terminals(session, true),
    enabled: open,
    staleTime: 0,
    refetchInterval: open ? 5000 : false,
    refetchIntervalInBackground: false,
  })
  const key = st.key
  const { viewport, epoch, dirty, revision } = st
  const { data } = useQuery({
    queryKey: [...queryKeys.ptyScreen(key?.session ?? session, key?.job ?? ''), viewport],
    queryFn: async () => ({
      reply: await api.pty_screen(key!.session, key!.job, viewport, dirty ? null : revision),
      origin: { queryKey: { key: key!, viewport }, epoch },
    }),
    enabled: open && !!key,
    // One request in flight per key (an interval tick joins the pending fetch); 500 ms while watching, 60 ms while typing at the live screen; none while hidden or closed.
    refetchInterval: typing && viewport.anchor === 'live' ? 60 : 500,
    refetchIntervalInBackground: false,
    retry: false,
    staleTime: 0,
    gcTime: 0,
  })
  useEffect(() => {
    if (data) dispatch({ type: 'reply', reply: data.reply, origin: data.origin })
  }, [data])
  const first = terminals[0]
  useEffect(() => {
    if (!key && first) dispatch({ type: 'select', key: { harness, session: first.session, job: first.id } })
  }, [key, first, harness])
  const cols = st.page?.cols ?? 120
  useEffect(() => {
    const el = screen.current
    if (!el) return
    const ro = new ResizeObserver(() => setFontSize(fit(el.clientWidth, cols)))
    ro.observe(el)
    return () => ro.disconnect()
  }, [cols])
  const live = !!st.page && !st.page.exit && !st.unavailable
  useEffect(() => {
    if (!live && typing) input.current?.blur()
  }, [live, typing])
  // One call in flight at a time: keystrokes reach the process in typing order.
  const lane = useRef<Promise<unknown>>(Promise.resolve())
  const type = (text: string | null, keys: Key[]) => {
    if (!key || (!text && keys.length === 0)) return
    lane.current = lane.current.then(() =>
      api.pty_input(key.session, key.job, text, keys).then(
        () => queryClient.invalidateQueries({ queryKey: queryKeys.ptyScreen(key.session, key.job) }),
        () => undefined,
      ),
    )
  }

  const current = terminals.find((t) => t.id === key?.job && t.session === key?.session)
  const page = st.page
  const nav = (n: Nav) => dispatch({ type: 'nav', nav: n })
  const status = [
    current ? `${current.name ?? current.command} · ${current.state}` : key ? key.job : 'No terminals',
    page && `revision ${Number(page.revision)}`,
    page?.exit && (typeof page.exit === 'string' ? page.exit : `exit ${page.exit.exited}`),
    page && page.redaction_complete === false && 'redaction incomplete at retained boundary',
    st.unavailable && 'No longer available.',
    st.daemonRestarted && 'Daemon restarted.',
  ]
    .filter(Boolean)
    .join(' · ')
  const where =
    page?.viewport.anchor === 'history'
      ? `rows ${page.origin}–${page.origin + page.lines.length} of ${page.retained}`
      : page?.alternate
        ? 'Live (alternate screen)'
        : 'Live'

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2 p-2">
      <div className="flex flex-wrap items-center gap-2">
        <select
          aria-label="Terminal"
          className="h-9 min-w-0 flex-1 rounded-md border border-border-subtle bg-background px-2 text-sm pointer-coarse:h-11"
          value={key ? `${key.session}/${key.job}` : ''}
          onChange={(e) => {
            const t = terminals.find((x) => `${x.session}/${x.id}` === e.target.value)
            if (t) dispatch({ type: 'select', key: { harness, session: t.session, job: t.id } })
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
        <div className="flex items-center gap-1" role="group" aria-label="History">
          <Button variant="outline" size="icon-sm" aria-label="Page up" title="Page up (PageUp)" disabled={!page} onClick={() => nav('pageUp')}>
            <ChevronsUp />
          </Button>
          <Button variant="outline" size="icon-sm" aria-label="Up one row" title="Up (↑)" disabled={!page} onClick={() => nav('up')}>
            <ChevronUp />
          </Button>
          <Button variant="outline" size="icon-sm" aria-label="Down one row" title="Down (↓)" disabled={!page} onClick={() => nav('down')}>
            <ChevronDown />
          </Button>
          <Button variant="outline" size="icon-sm" aria-label="Page down" title="Page down (PageDown)" disabled={!page} onClick={() => nav('pageDown')}>
            <ChevronsDown />
          </Button>
          <Button
            variant={viewport.anchor === 'live' ? 'secondary' : 'outline'}
            size="sm"
            className="pointer-coarse:h-11"
            title="Live (End)"
            onClick={() => nav('live')}
          >
            Live
          </Button>
        </div>
        <Button
          variant={typing ? 'secondary' : 'outline'}
          size="sm"
          className="pointer-coarse:h-11"
          aria-pressed={typing}
          title={typing ? 'Stop typing into the terminal (Ctrl+])' : 'Type into the terminal'}
          disabled={!live}
          onClick={() => (typing ? input.current?.blur() : input.current?.focus())}
        >
          <Keyboard />
          {typing ? 'Typing' : 'Type'}
        </Button>
      </div>
      <div
        ref={screen}
        tabIndex={typing ? -1 : 0}
        aria-label={typing ? 'Terminal screen; keys are sent to the program' : 'Terminal screen; arrow and page keys scroll history'}
        className={cn(
          'relative min-h-0 flex-1 overflow-auto rounded-md outline-none focus-visible:ring-2 focus-visible:ring-ring',
          typing && 'ring-2 ring-ring',
        )}
        style={{ backgroundColor: BG }}
        onKeyDown={(e) => {
          if (typing) return
          const n = NAV_KEYS[e.key]
          if (!n || !page) return
          e.preventDefault()
          nav(n)
        }}
      >
        {/* Focus target: gives IME composition, soft keyboards and paste for free; never painted. */}
        <textarea
          ref={input}
          aria-label="Type into the terminal"
          className="absolute top-0 left-0 size-px opacity-0"
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
          tabIndex={-1}
          onFocus={() => setTyping(true)}
          onBlur={() => setTyping(false)}
          onKeyDown={(e) => {
            if (e.key === ']' && e.ctrlKey) {
              e.preventDefault()
              e.currentTarget.blur()
              return
            }
            const i = keyToInput(e)
            if (!i) return
            e.preventDefault()
            e.stopPropagation()
            type(i.text, i.keys)
          }}
          onCompositionEnd={(e) => {
            type(e.data || null, [])
            e.currentTarget.value = ''
          }}
          onInput={(e) => {
            // Soft keyboards report `Unidentified` keys; the text lands here instead.
            if (e.nativeEvent instanceof InputEvent && e.nativeEvent.isComposing) return
            const v = e.currentTarget.value
            e.currentTarget.value = ''
            type(v || null, [])
          }}
          onPaste={(e) => {
            e.preventDefault()
            const t = e.clipboardData.getData('text')
            if (t) type(null, [{ paste: t }])
          }}
        />
        {page ? (
          <Screen page={page} fontSize={fontSize} />
        ) : (
          <p className="p-4 text-sm" style={{ color: FG }}>
            {st.unavailable ? 'No longer available.' : key ? 'Connecting…' : 'Pick a terminal.'}
          </p>
        )}
      </div>
      {mobile && typing && (
        <div className="flex gap-1 overflow-x-auto" role="group" aria-label="Terminal keys">
          {TOOLBAR.map(([label, k]) => (
            <Button
              key={label}
              variant="outline"
              size="sm"
              className="h-11 min-w-11 font-mono"
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => type(null, [k])}
            >
              {label}
            </Button>
          ))}
        </div>
      )}
      <p aria-live="polite" className={cn('truncate text-xs text-fg-secondary', st.unavailable && 'text-warning')} title={status}>
        {where} · {status}
      </p>
    </div>
  )
}

/** The session's PTY jobs (51 §7.3, 55 §4): watch, or type into one. Dialog on desktop, full-height sheet on mobile. Polls only while open. */
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
          {open && <Body session={session} open={open} />}
        </SheetContent>
      </Sheet>
    )
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex h-[85vh] max-w-[95vw] flex-col gap-0 p-0 pt-10 sm:max-w-[95vw]">
        <DialogTitle className="sr-only">Terminals</DialogTitle>
        {open && <Body session={session} open={open} />}
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
