// Owner: U1
import { useQuery } from '@tanstack/react-query'
import { openPath } from '@tauri-apps/plugin-opener'
import { AlertTriangle, Check, ChevronDown, Circle, Copy, Eye, EyeOff, FileText, History, Loader2, MessageSquare, Play, Square, X } from 'lucide-react'
import { createContext, lazy, type ReactNode, Suspense, useContext, useEffect, useRef, useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { isApple } from '@/lib/chord'
import { isDesktop, touch } from '@/lib/platform'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { CellRef } from '@/rpc/bindings/CellRef'
import type { CellRun } from '@/rpc/bindings/CellRun'
import { useConnection } from '@/store/connection'
import { AnsiText, CellContainer, CellOutputs, ExecutionCount } from './Outputs'
import { clock, duration, ended, errorText, exitText, refKey, scopeOf } from './store'
import type { CellProps, CellViewState } from './types'
import { labelOf, titleOf } from './view'
// xterm loads with the first live run.
const Terminal = lazy(() => import('@/components/chat/TerminalViewer').then((m) => ({ default: m.Terminal })))

/** `cells.ui_tail_lines` (§8.6, §15.4): the finished view's last lines; the rest is one click away. */
export const TAIL_LINES = 200
/** `cells.kill_after_secs` (W-p): Kill is enabled this long after Stop. */
export const KILL_AFTER_MS = 5000

/**
 * What the host (chat `CellFence`, note `NoteCell`) knows beyond `CellProps`: whether the cell starts unfolded (the
 * latest cell of a reply stays open, §11.1), the `after:` cell's label, the queue position, a refusal or read error
 * to show, a keyboard hint, and `pending` — the reply is still being written ("ready when the reply ends", §4.1).
 */
export type CellExtras = { open?: boolean; afterLabel?: string; queuePos?: number; notice?: string | null; hint?: string | null; pending?: boolean }
export const CellExtras = createContext<CellExtras>({})

/** DOM id of a cell (the "Replaced by a newer version ↓" link, `?cell=` scroll). */
export const cellDomId = (ref: CellRef) => `cell-${refKey(ref).replace(/[^A-Za-z0-9_-]/g, '_')}`

/** `~/dr-t6/frp-integration/…/sa-frp-integration`: the head and the folder, the middle elided. */
export function middleEllipsis(path: string, max = 44): string {
  if (path.length <= max) return path
  const parts = path.split('/')
  const tail = parts.at(-1) ?? ''
  const head = parts.slice(0, 3).join('/')
  return head.length + tail.length + 2 <= max ? `${head}/…/${tail}` : `…/${tail}`
}

const ordinal = (n: number) => `${n}${n % 10 === 1 && n !== 11 ? 'st' : n % 10 === 2 && n !== 12 ? 'nd' : n % 10 === 3 && n !== 13 ? 'rd' : 'th'}`
const expectText = (run: CellRun) => {
  const met = run.expect.filter((e) => e.met).length
  return met === run.expect.length ? `expect ✓ ${met}/${run.expect.length}` : `expect ✗ ${run.expect.length - met}/${run.expect.length} missed`
}
const succeeded = (run: CellRun) => run.state === 'done'

/** Seconds since `from`, ticking while `active`. */
function useElapsed(from: number | null, active: boolean): number {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!active) return
    const t = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(t)
  }, [active])
  return from === null ? 0 : Math.max(0, (now - from) / 1000)
}

function StateDot({ view }: { view: CellViewState }) {
  const cls = 'size-3.5 shrink-0'
  switch (view) {
    case 'running':
      return <Loader2 className={cn(cls, 'text-fg-accent motion-safe:animate-spin')} aria-label="running" />
    case 'done':
      return <Check className={cn(cls, 'text-success')} aria-label="done" />
    case 'failed':
      return <X className={cn(cls, 'text-danger')} aria-label="failed" />
    case 'stopped':
      return <Square className={cn(cls, 'fill-current text-fg-secondary')} aria-label="stopped" />
    case 'lost':
    case 'invalid':
      return <AlertTriangle className={cn(cls, 'text-warning')} aria-label={view} />
    default:
      return <Circle className={cn(cls, 'text-fg-tertiary', view === 'queued' && 'text-fg-accent')} aria-label={view} />
  }
}

function Chip({ children, tone, mono, title }: { children: ReactNode; tone?: 'danger' | 'warning' | 'success' | 'info'; mono?: boolean; title?: string }) {
  return (
    <Badge
      variant="outline"
      title={title}
      className={cn(
        'h-5 max-w-full shrink truncate rounded-md border-border-subtle px-1.5 text-[11px] font-normal text-fg-secondary',
        mono && 'font-mono',
        tone === 'danger' && 'border-danger-border bg-danger-bg text-danger',
        tone === 'warning' && 'border-warning-border bg-warning-bg text-warning',
        tone === 'success' && 'border-success-border bg-success-bg text-success',
        tone === 'info' && 'border-info-border bg-info-bg text-info',
      )}
    >
      {children}
    </Badge>
  )
}

function Notice({ tone, children }: { tone: 'danger' | 'warning' | 'success' | 'info' | 'muted'; children: ReactNode }) {
  return (
    <div
      className={cn(
        'flex flex-wrap items-center gap-x-2 px-3 py-1.5 text-xs',
        tone === 'danger' && 'bg-danger-bg text-danger',
        tone === 'warning' && 'bg-warning-bg text-warning',
        tone === 'success' && 'bg-success-bg text-success',
        tone === 'info' && 'bg-info-bg text-info',
        tone === 'muted' && 'text-fg-tertiary',
      )}
    >
      {children}
    </div>
  )
}

/** The finished run's text: the last `TAIL_LINES` lines, "show all N lines" pages the rest through `cell_output`. */
function StaticText({ run, text }: { run: CellRun; text: string }) {
  const api = useApi()
  const [all, setAll] = useState<string | null>(null)
  const lines = text.split('\n')
  const shown = all ?? (lines.length > TAIL_LINES ? lines.slice(-TAIL_LINES).join('\n') : text)
  const loadAll = async () => {
    let out = ''
    for (let offset: number | null = 0; offset !== null; ) {
      const page = await api.cell_output(run.id, offset, 256 * 1024, true)
      out += page.text
      offset = page.next
    }
    setAll(out)
  }
  return (
    <>
      {all === null && lines.length > TAIL_LINES && (
        <Button variant="link" size="sm" className="h-6 px-0 text-xs" onClick={() => void loadAll()}>
          show all {lines.length.toLocaleString()} lines
        </Button>
      )}
      <AnsiText text={shown} className="max-h-[28rem] overflow-auto text-xs" />
    </>
  )
}

/** The output of a run that is not live: the record's outputs (`cell_run_get`), shell text as ANSI, rich ones through the media router. */
function RunOutput({ run }: { run: CellRun }) {
  const api = useApi()
  const q = useQuery({ queryKey: ['cells', 'run', run.id, run.state, run.ended_ms], queryFn: () => api.cell_run_get(run.id), staleTime: Infinity })
  if (q.error) return <p className="text-xs text-danger">{errorText(q.error)}</p>
  if (!q.data) return <p className="text-xs text-fg-tertiary">loading output…</p>
  const outputs = q.data.outputs
  if (outputs.length === 0) return <p className="text-xs text-fg-tertiary">no output</p>
  if (outputs.every((o) => o.output_type === 'stream')) return <StaticText run={run} text={outputs.map((o) => o.text).join('')} />
  return <CellOutputs run={run.id} outputs={outputs} />
}

/** The live shell run: the kernel PTY from mark C (`cell_attach`), typed through `cell_input` (§7.4). */
function LiveTerminal({ run, code, label }: { run: CellRun; code: string; label: string }) {
  const first = code.split('\n').find((l) => l.trim()) ?? ''
  return (
    <div className="flex flex-col gap-1">
      <div className="rounded-md p-0.5 ring-1 ring-fg-accent">
        <p className="truncate px-2 pt-1 font-mono text-xs text-fg-tertiary">$ {first}</p>
        <div className="flex h-[32rem] max-h-[60vh] flex-col">
          <Suspense fallback={null}>
            <Terminal source={{ kind: 'cell', run: run.id }} label={label} />
          </Suspense>
        </div>
      </div>
      <p className="text-xs text-fg-accent">the terminal is live — type or use arrows</p>
    </div>
  )
}

/** Copy the run's clean text (`cell_output`, no ANSI, §8.6). */
function CopyOutput({ run }: { run: CellRun }) {
  const api = useApi()
  const [done, setDone] = useState(false)
  const copy = async () => {
    let text = ''
    for (let offset: number | null = 0; offset !== null; ) {
      const page = await api.cell_output(run.id, offset, 256 * 1024, false)
      text += page.text
      offset = page.next
    }
    await navigator.clipboard.writeText(text)
    setDone(true)
    setTimeout(() => setDone(false), 2000)
  }
  return (
    <Button variant="ghost" size="sm" className="h-7 gap-1 px-2 text-xs pointer-coarse:h-12" onClick={() => void copy().catch(() => undefined)}>
      {done ? <Check className="size-3.5" aria-hidden /> : <Copy className="size-3.5" aria-hidden />}
      copy
    </Button>
  )
}

/** history ▾: this cell's runs (`cell_runs` of its scope, filtered to the cell); picking one shows its output. */
function HistoryMenu({ cellRef, onPick }: { cellRef: CellRef; onPick(run: CellRun): void }) {
  const api = useApi()
  const [open, setOpen] = useState(false)
  const key = refKey(cellRef)
  const q = useQuery({ queryKey: ['cells', 'runs', key], queryFn: () => api.cell_runs(scopeOf(cellRef), 50), enabled: open })
  const runs = (q.data ?? []).filter((r) => refKey(r.cell) === key)
  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="sm" className="h-7 gap-1 px-2 text-xs text-fg-secondary pointer-coarse:h-12">
          <History className="size-3.5" aria-hidden />
          history
          <ChevronDown className="size-3" aria-hidden />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        {q.error && <DropdownMenuItem disabled>{errorText(q.error)}</DropdownMenuItem>}
        {q.data && runs.length === 0 && <DropdownMenuItem disabled>no runs yet</DropdownMenuItem>}
        {runs.map((r) => (
          <DropdownMenuItem key={r.id} onSelect={() => onPick(r)} className="font-mono text-xs">
            {clock(r.queued_ms)} · {exitText(r)}
            {r.secs != null && ` · ${duration(r.secs)}`}
            {r.edited && ' · edited'}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

/** 171 §11.1: the one cell component for chat, notes and notebooks. */
export function Cell({ cellRef, cell, code, runnable, view, state, latest, host, callbacks, editor, readOnly }: CellProps) {
  const extras = useContext(CellExtras)
  const dataDir = useConnection((s) => s.info?.data_dir)
  const label = labelOf(cell)
  const title = titleOf(cell, cellRef.type === 'chat' ? cellRef.ordinal : undefined)
  const o = cell.options
  const [picked, setPicked] = useState<CellRun | null>(null)
  const run = picked ?? latest
  const live = latest?.state === 'running' || latest?.state === 'queued'
  const [showOutput, setShowOutput] = useState<boolean | null>(null)
  const [folded, setFolded] = useState<boolean | null>(null)
  const [raw, setRaw] = useState(false)
  const [focused, setFocused] = useState(false)
  const [stoppedAt, setStoppedAt] = useState<number | null>(null)
  const elapsed = useElapsed(latest?.started_ms ?? null, latest?.state === 'running')
  const sinceStop = useElapsed(stoppedAt, stoppedAt !== null && live)
  const killReady = stoppedAt !== null && sinceStop * 1000 >= KILL_AFTER_MS
  useEffect(() => {
    if (!live) setStoppedAt(null)
  }, [live])

  // §11.9: state changes are announced politely from the footer.
  const [announce, setAnnounce] = useState('')
  const seen = useRef(latest ? `${latest.id}:${latest.state}` : '')
  useEffect(() => {
    const now = latest ? `${latest.id}:${latest.state}` : ''
    if (now === seen.current) return
    seen.current = now
    if (!latest) return
    if (latest.state === 'running') setAnnounce(`${title} running`)
    else if (ended(latest))
      setAnnounce(`${title} finished, ${exitText(latest)}${latest.expect.length ? `, expect ${latest.expect.every((e) => e.met) ? 'met' : 'missed'}` : ''}`)
  }, [latest, title])

  const problems = state?.problems ?? []
  const errors = problems.filter((p) => p.severity === 'error')
  const warnings = problems.filter((p) => p.severity === 'warning')
  const cwdMissing = warnings.find((p) => p.code === 'cwd_not_found')
  // A pending reply shows the button disabled with its reason ("ready when the reply ends"), not no button.
  const canPress = (runnable || !!extras.pending) && !readOnly && view !== 'invalid' && view !== 'superseded' && view !== 'guest'
  const stop = () => {
    if (!latest || !live) return
    if (stoppedAt === null) {
      setStoppedAt(Date.now())
      callbacks.onStop(false)
    } else if (killReady) callbacks.onStop(true)
  }
  const isFolded = folded ?? (!extras.open && view === 'done' && !picked)
  const outputOpen = showOutput ?? (view !== 'done' || !!extras.open || !!picked)

  const mod = isApple() ? '⌘' : 'Ctrl'
  const phoneDanger = touch() && o.danger
  const primary = (() => {
    if (!canPress) return null
    if (live)
      return (
        <div className="flex items-center gap-1">
          <Button size="sm" variant="outline" className="h-7 gap-1 pointer-coarse:h-12" onClick={stop} disabled={stoppedAt !== null} title={`Stop (Ctrl-C) · ${mod}.`}>
            <Square className="size-3 fill-current" aria-hidden />
            Stop
          </Button>
          {latest?.state === 'running' && (
            <Button
              size="sm"
              variant="outline"
              className="h-7 pointer-coarse:h-12"
              disabled={!killReady}
              aria-description={killReady ? undefined : 'Kill is enabled 5 s after Stop'}
              onClick={() => callbacks.onStop(true)}
            >
              Kill
            </Button>
          )}
        </div>
      )
    const text = view === 'disarmed' ? `after: ${extras.afterLabel ?? o.after}` : view === 'lost' ? 'Run again' : label
    const reason = view === 'disarmed' ? `arms when ${extras.afterLabel ?? o.after} has succeeded on its current code` : extras.pending ? 'ready when the reply ends' : undefined
    const disabled = view === 'disarmed' || !!extras.pending || !!cwdMissing
    return (
      <Button
        size="sm"
        variant={o.danger ? 'destructive' : 'default'}
        className={cn('h-7 gap-1 pointer-coarse:h-12', phoneDanger && 'w-full')}
        aria-disabled={disabled || undefined}
        aria-description={reason}
        title={reason}
        onClick={() => !disabled && callbacks.onRun({ type: 'cell' })}
      >
        {!o.danger && <Play className="size-3" aria-hidden />}
        {text}
      </Button>
    )
  })()

  const header = (
    <div className="flex flex-wrap items-center gap-1.5 px-3 py-2">
      <StateDot view={view} />
      <span className={cn('mr-1 text-sm font-medium', view === 'superseded' && 'text-fg-tertiary')}>{title}</span>
      {o.danger && <Chip tone="danger">danger</Chip>}
      <Chip>{state?.interpreter ? [state.interpreter.kind, state.interpreter.version, `(${state.interpreter.source})`].filter(Boolean).join(' ') : cell.lang}</Chip>
      {o.cwd && (
        <Chip mono title={o.cwd} tone={cwdMissing ? 'warning' : undefined}>
          {middleEllipsis(o.cwd)}
          {cwdMissing && ' not found'}
        </Chip>
      )}
      {host && <Chip tone="info">on {host.label}</Chip>}
      {o.name && o.label && <Chip mono>{o.name}</Chip>}
      {o.after && <Chip>after: {o.after}</Chip>}
      {o.expect.map((e, i) => (
        <Chip key={i} title="expect">
          expect: {e.kind === 'regex' ? `/${e.value}/` : e.value}
        </Chip>
      ))}
      {o.private && <Chip>private</Chip>}
      {(state?.chips ?? []).map((c, i) => (
        <Chip key={`c${i}`} tone={c.stops_run_all ? 'warning' : undefined} title={c.stops_run_all ? 'Run all stops before this cell' : undefined}>
          {c.text}
        </Chip>
      ))}
      {warnings
        .filter((p) => p !== cwdMissing)
        .map((p, i) => (
          <Chip key={`w${i}`} tone="warning" title={p.code}>
            {p.message}
          </Chip>
        ))}
      {view === 'edited' && (
        <Chip>
          <i>edited</i>
        </Chip>
      )}
      {view === 'queued' && <Chip tone="info">queued{extras.queuePos ? ` · ${ordinal(extras.queuePos)}` : ''}</Chip>}
      {latest?.state === 'running' && <span className="font-mono text-xs text-fg-accent tabular-nums">running {duration(elapsed).replace(' s', 's')}</span>}
      <span className="ml-auto flex items-center gap-1">
        {cell.lang === 'python' && run && <ExecutionCount count={null} isExecuting={latest?.state === 'running'} />}
        <Button variant="ghost" size="sm" className="h-7 px-2 text-xs text-fg-tertiary pointer-coarse:h-12" aria-pressed={raw} onClick={() => setRaw(!raw)}>
          #|
        </Button>
        {!phoneDanger && primary}
      </span>
    </div>
  )

  const notices = (
    <>
      {errors.map((p, i) => (
        <Notice key={i} tone="danger">
          {p.message}
        </Notice>
      ))}
      {cwdMissing && <Notice tone="warning">Folder not found — this cell will not start anywhere else. It arms when the folder exists (an earlier cell may create it).</Notice>}
      {state?.superseded_by && (
        <Notice tone="muted">
          <a href={`#${cellDomId(state.superseded_by)}`} className="underline underline-offset-2">
            Replaced by a newer version ↓
          </a>
        </Notice>
      )}
      {state?.member_edit && <Notice tone="warning">Changed by {state.member_edit} since your last run</Notice>}
      {state?.armed && (
        <Notice tone="success">
          {`armed by ${(extras.afterLabel ?? o.after ?? '').toLowerCase()} ✓ ${clock(state.armed.at_ms)} · ${state.armed.summary}`}
        </Notice>
      )}
      {view === 'lost' && <Notice tone="warning">lost — engine restarted{latest?.cwd ? ` · ${latest.cwd}` : ''}</Notice>}
      {view === 'stale' && <Notice tone="muted">ran an earlier version</Notice>}
      {view === 'edited' && <Notice tone="muted">theuth will see the code that actually ran.</Notice>}
      {view === 'stopped' && <Notice tone="muted">stopped by you</Notice>}
      {view === 'guest' && <Notice tone="muted">Only the owner can run cells</Notice>}
      {extras.pending && <Notice tone="muted">ready when the reply ends</Notice>}
      {extras.notice && <Notice tone="danger">{extras.notice}</Notice>}
      {extras.hint && <Notice tone="info">{extras.hint}</Notice>}
    </>
  )

  const footer = run && ended(run) && (
    <div className="flex flex-wrap items-center gap-1.5 px-3 py-1.5 text-xs text-fg-secondary">
      <Chip tone={succeeded(run) ? 'success' : run.state === 'failed' ? 'danger' : undefined}>
        {succeeded(run) ? '✓' : '✗'} {exitText(run)}
      </Chip>
      {run.secs != null && <span className="tabular-nums">{duration(run.secs)}</span>}
      {run.expect.length > 0 && <Chip tone={run.expect.every((e) => e.met) ? 'success' : 'danger'}>{expectText(run)}</Chip>}
      {run.edited && <i>edited</i>}
      {picked && <span>run of {clock(picked.queued_ms)}</span>}
      {o.private && <span>theuth got exit and expect only</span>}
      <span className="ml-auto flex items-center gap-0.5">
        {!readOnly && view !== 'guest' && (
          <Button variant="ghost" size="sm" className="h-7 gap-1 px-2 text-xs pointer-coarse:h-12" onClick={callbacks.onAsk}>
            <MessageSquare className="size-3.5" aria-hidden />
            Ask theuth
          </Button>
        )}
        <Button variant="ghost" size="sm" className="h-7 gap-1 px-2 text-xs pointer-coarse:h-12" onClick={() => setShowOutput(!outputOpen)} aria-expanded={outputOpen}>
          {outputOpen ? <EyeOff className="size-3.5" aria-hidden /> : <Eye className="size-3.5" aria-hidden />}
          {outputOpen ? 'hide output' : 'show output'}
        </Button>
        {isDesktop() && dataDir && (
          <Button variant="ghost" size="sm" className="h-7 gap-1 px-2 text-xs pointer-coarse:h-12" onClick={() => void openPath(`${dataDir}/cells/runs/${run.id}`).catch(() => undefined)}>
            <FileText className="size-3.5" aria-hidden />
            Open log
          </Button>
        )}
        <CopyOutput run={run} />
        {!readOnly && <HistoryMenu cellRef={cellRef} onPick={(r) => setPicked(r.id === latest?.id ? null : r)} />}
      </span>
    </div>
  )

  const output = live && latest ? (
    latest.state === 'running' && latest.kernel.spec.type === 'shell' ? (
      <LiveTerminal run={latest} code={code} label={label} />
    ) : (
      <p className="text-xs text-fg-tertiary">{latest.state === 'queued' ? 'waiting for the kernel…' : 'running…'}</p>
    )
  ) : run && ended(run) && outputOpen ? (
    <RunOutput run={run} />
  ) : null

  const id = cellDomId(cellRef)
  const shell = cn(
    'my-4 overflow-hidden rounded-xl border border-border bg-layer-1',
    o.danger && 'border-[1.5px] border-danger-border',
    view === 'superseded' && 'opacity-60',
  )
  const dataAttrs = { 'data-cell': '', 'data-cell-name': o.name ?? undefined, 'data-cell-ordinal': cellRef.type === 'chat' ? cellRef.ordinal : undefined }

  if (isFolded && run)
    return (
      <div id={id} {...dataAttrs} className={shell}>
        <button
          type="button"
          className="flex w-full flex-wrap items-center gap-2 px-3 py-2 text-left text-sm hover:bg-layer-2 pointer-coarse:min-h-12"
          onClick={() => setFolded(false)}
          aria-expanded={false}
        >
          <StateDot view={view} />
          <span className="font-medium">{title}</span>
          <span className="font-mono text-xs text-fg-secondary">
            {exitText(run)}
            {run.secs != null && ` · ${duration(run.secs)}`}
            {run.expect.length > 0 && ` · ${run.expect.every((e) => e.met) ? 'expect ✓' : 'expect ✗'}`}
          </span>
          {run.summary && <span className="min-w-0 flex-1 truncate text-xs text-fg-tertiary">{run.summary}</span>}
          <span className="ml-auto text-xs text-fg-secondary underline-offset-2 hover:underline">show output</span>
        </button>
      </div>
    )

  return (
    <div
      id={id}
      {...dataAttrs}
      className={shell}
      onFocus={() => setFocused(true)}
      onBlur={(e) => !e.currentTarget.contains(e.relatedTarget) && setFocused(false)}
      onKeyDown={(e) => {
        // §11.7: Mod+. stops; again once Kill is enabled kills.
        if (e.key === '.' && (isApple() ? e.metaKey : e.ctrlKey)) {
          e.preventDefault()
          stop()
        }
      }}
    >
      <CellContainer
        id={refKey(cellRef)}
        cellType="code"
        isFocused={focused}
        // Chat cells have no state lane: the notebook column insets collapse to the card's padding.
        className="[--cell-content-column-inset:0px] sm:[--cell-content-column-inset:0px] [--cell-output-row-inset:0.75rem] sm:[--cell-output-row-inset:0.75rem]"
        isPreviousCellFromFocused
        codeContent={
          <>
            {header}
            {notices}
            {raw && (
              <pre className="border-y border-border-subtle px-3 py-1 font-mono text-xs text-fg-tertiary">
                {cell.header.map((h) => `#| ${h.key}${h.value === null ? '' : `: ${h.value}`}`).join('\n')}
              </pre>
            )}
            <div className={cn('border-t border-border-subtle bg-layer-2/40', view === 'superseded' && 'pointer-events-none')}>{editor}</div>
            {phoneDanger && primary && <div className="p-2">{primary}</div>}
            {footer}
            {folded === false && run && view === 'done' && (
              <Button variant="ghost" size="sm" className="mx-2 mb-1 h-7 px-2 text-xs" onClick={() => setFolded(true)}>
                fold
              </Button>
            )}
          </>
        }
        outputContent={output ?? undefined}
      />
      <p aria-live="polite" className="sr-only">
        {announce}
      </p>
    </div>
  )
}
