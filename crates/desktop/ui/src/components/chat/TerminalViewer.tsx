import { useQuery } from '@tanstack/react-query'
import { ChevronDown, ChevronUp, ChevronsDown, ChevronsUp, SquareTerminal } from 'lucide-react'
import { type CSSProperties, Fragment, useEffect, useReducer, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog'
import { Sheet, SheetClose, SheetContent, SheetTitle } from '@/components/ui/sheet'
import { useIsMobile } from '@/hooks/use-mobile'
import { queryKeys, useApi } from '@/lib/rpc'
import { EMPTY, type Nav, css, reduce } from '@/lib/terminal'
import { cn } from '@/lib/utils'
import type { Cell } from '@/rpc/bindings/Cell'
import type { PtyPage } from '@/rpc/bindings/PtyPage'
import type { Style } from '@/rpc/bindings/Style'
import type { StyledRun } from '@/rpc/bindings/StyledRun'
import { useScope } from '@/store/harness-scope'

// Fixed dark canvas so the 16-colour palette reads on either app theme.
const BG = '#1e1e1e'
const FG = '#e5e5e5'

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

/** One page as a fixed-cell grid: a `<pre>` of styled runs, the cursor cell inverted, wide continuations skipped. */
export function Screen({ page }: { page: PtyPage }) {
  return (
    <pre
      className="m-0 min-w-max select-text p-2 font-mono text-xs leading-[1.3] whitespace-pre"
      style={{ backgroundColor: BG, color: FG, minHeight: `${page.lines.length}lh` }}
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

function Body({ session, open }: { session: string; open: boolean }) {
  const api = useApi()
  const harness = useScope().id
  const [st, dispatch] = useReducer(reduce, EMPTY)
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
    // One request in flight per key (an interval tick joins the pending fetch), at most one every 500 ms, none while hidden or closed.
    refetchInterval: 500,
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
      </div>
      <div
        tabIndex={0}
        aria-label="Terminal screen; arrow and page keys scroll history"
        className="min-h-0 flex-1 overflow-auto rounded-md outline-none focus-visible:ring-2 focus-visible:ring-ring"
        style={{ backgroundColor: BG }}
        onKeyDown={(e) => {
          const n = NAV_KEYS[e.key]
          if (!n || !page) return
          e.preventDefault()
          nav(n)
        }}
      >
        {page ? (
          <Screen page={page} />
        ) : (
          <p className="p-4 text-sm" style={{ color: FG }}>
            {st.unavailable ? 'No longer available.' : key ? 'Connecting…' : 'Pick a terminal.'}
          </p>
        )}
      </div>
      <p aria-live="polite" className={cn('truncate text-xs text-fg-secondary', st.unavailable && 'text-warning')} title={status}>
        {where} · {status}
      </p>
    </div>
  )
}

/** Read-only view of the session's PTY jobs (51 §7.3): dialog on desktop, full-height sheet on mobile. Polls only while open. */
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

/** Header icon; `/terminals` reaches it through the `wth:terminals` window event. */
export function TerminalButton({ session }: { session: string }) {
  const [open, setOpen] = useState(false)
  useEffect(() => {
    const h = (e: Event) => {
      if ((e as CustomEvent<{ session?: string }>).detail?.session === session) setOpen(true)
    }
    window.addEventListener('wth:terminals', h)
    return () => window.removeEventListener('wth:terminals', h)
  }, [session])
  return (
    <>
      <Button variant="ghost" className="size-9 rounded-lg text-fg-secondary" aria-label="Terminals" onClick={() => setOpen(true)}>
        <SquareTerminal className="size-5" />
      </Button>
      <TerminalViewer session={session} open={open} onOpenChange={setOpen} />
    </>
  )
}
