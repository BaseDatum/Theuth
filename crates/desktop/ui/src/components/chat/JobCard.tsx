import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import type { ThemedToken } from 'shiki'
import { useQuery } from '@tanstack/react-query'
import { ChevronRight, Copy } from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { AnsiText, ansiToPlainText } from '@/components/cells/vendor/nteract/outputs/ansi-output'
import { queryKeys, useApi } from '@/lib/rpc'
import { loadHighlighter } from '@/lib/shiki'
import { disclosureKey, useDisclosure, useToolRuns } from '@/lib/tool-runs'
import { cn } from '@/lib/utils'
import { rows } from '@/lib/jobTail'
import { useSessions, type Entry } from '@/store/sessions'
import type { JobLogPage, JobView } from '@/rpc/bindings/method.agent.output'
import { useQuerySupport } from '@/rpc/capabilities'
import { toolKey } from './disclosure'
import { useTranscriptJump } from './jump'
import { PopoutTerminalButton, Terminal } from './TerminalViewer'

const PAGE = 32 * 1024
const EMPTY: Entry[] = []
export const jobFailed = (job: JobView) => job.state !== 'running' && job.state !== 'exited 0'
const clock = (ms: number | null | undefined) => ms == null ? 'unknown' : new Date(ms).toLocaleTimeString()
export const jobBytes = (n: number) => n < 1024 ? `${n} B` : n < 1024 * 1024 ? `${(n / 1024).toFixed(1)} KiB` : `${(n / (1024 * 1024)).toFixed(1)} MiB`
const duration = (secs: number) => secs < 60 ? `${secs}s` : `${Math.floor(secs / 60)}m ${secs % 60}s`

export function JobState({ job }: { job: JobView }) {
  const running = job.state === 'running'
  const label = running ? 'running' : job.state === 'lost' ? 'lost · engine restarted' : job.state === 'timeout' ? `timeout · ${job.secs}s` : job.state.replace(/^exited /, 'exit ')
  return <span className={cn('inline-flex items-center rounded-full border border-current px-2 py-0.5 font-mono text-[11px] whitespace-nowrap', running ? 'text-project-lapis' : job.state === 'exited 0' ? 'bg-success-bg text-success' : job.state.startsWith('exited ') ? 'bg-danger-bg text-danger' : job.state === 'timeout' ? 'text-project-ochre' : 'text-fg-tertiary')}>
    {running && <span className="mr-1.5 size-1.5 animate-pulse rounded-full bg-project-lapis" />}{label}
  </span>
}

// Shiki supplies bash scopes, never its theme's RGB values: this surface uses
// only the harness minerals, in light and dark themes alike.
function tokenClass(token: ThemedToken) {
  const scopes = token.explanation?.flatMap((e) => e.scopes.map((s) => s.scopeName)).join(' ') ?? ''
  return /comment/.test(scopes) ? 'text-fg-tertiary' : /variable.parameter.option/.test(scopes) ? 'text-project-ochre' : /variable/.test(scopes) ? 'text-project-plum' : /keyword|storage/.test(scopes) ? 'text-project-lapis' : /entity.name|support.function/.test(scopes) ? 'text-project-faience' : /string.quoted|string.interpolated/.test(scopes) ? 'text-success' : /constant|numeric/.test(scopes) ? 'text-project-ochre' : 'text-fg-primary'
}

function Command({ command }: { command: string }) {
  const [tokens, setTokens] = useState<{ code: string; lines: ThemedToken[][] } | null>(null)
  useEffect(() => {
    let gone = false
    if (command.length <= 100_000) void loadHighlighter().then(async (h) => {
      await h.loadLanguage('shellscript')
      const lines = h.codeToTokens(command, { lang: 'shellscript', theme: 'github-light', includeExplanation: true }).tokens
      if (!gone) setTokens({ code: command, lines })
    }).catch(() => {}) // Missing grammar chunk: keep the copyable plain command.
    return () => { gone = true }
  }, [command])
  return <div className="flex items-start gap-2 rounded-lg bg-layer-2 px-2.5 py-1.5">
    <pre className="min-w-0 flex-1 font-mono text-xs leading-5 wrap-anywhere whitespace-pre-wrap">{tokens?.code === command ? tokens.lines.map((line, i) => <span key={i}>{i > 0 && '\n'}{line.map((t, j) => <span key={j} className={tokenClass(t)}>{t.content}</span>)}</span>) : command}</pre>
    <Button size="icon-xs" variant="ghost" aria-label="Copy command" onClick={() => void copy(command)}><Copy className="size-3" /></Button>
  </div>
}

async function copy(text: string) {
  try { await navigator.clipboard.writeText(text) } catch (e) { toast.error(String(e)) }
}

function Output({ job }: { job: JobView }) {
  const api = useApi()
  const supported = useQuerySupport(['job_log'])
  const [page, setPage] = useState<JobLogPage | null>(null)
  const current = useRef<JobLogPage | null>(null)
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(false)
  const [wrap, setWrap] = useDisclosure(disclosureKey('job', job.session, job.id, 'wrap'))
  const pane = useRef<HTMLPreElement>(null)
  const follow = useRef(true)
  const prepend = useRef<number | null>(null)
  const running = job.state === 'running'
  const commit = (next: JobLogPage) => { current.current = next; setPage(next) }
  useEffect(() => {
    if (!supported || job.pty) return
    let gone = false
    let timer: ReturnType<typeof setTimeout>
    const refresh = async () => {
      try {
        const next = await api.job_log(job.session, job.id, current.current?.end ?? null, PAGE)
        if (gone) return
        const prev = current.current
        // A truncated/replaced file starts a new retained stream.
        commit(prev && next.end >= prev.end ? { ...next, start: prev.start, text: prev.text + next.text } : next)
        setError('')
      } catch (e) { if (!gone) setError(e instanceof Error ? e.message : String(e)) }
      if (!gone && running) timer = setTimeout(() => void refresh(), 2000)
    }
    void refresh()
    return () => { gone = true; clearTimeout(timer) }
  }, [api, job.session, job.id, job.pty, running, supported])
  const text = job.pty ? job.tail : page?.text ?? job.tail
  useLayoutEffect(() => {
    const el = pane.current
    if (!el) return
    if (prepend.current !== null) { el.scrollTop += el.scrollHeight - prepend.current; prepend.current = null }
    else if (follow.current) el.scrollTop = el.scrollHeight
  }, [text])
  const earlier = async () => {
    if (!page || loading || page.start === 0) return
    setLoading(true)
    const end = page.start
    const start = Math.max(0, end - PAGE)
    try {
      const older = await api.job_log(job.session, job.id, start, end - start)
      const prev = current.current
      if (prev) {
        follow.current = false
        prepend.current = pane.current?.scrollHeight ?? null
        commit({ ...prev, start: older.start, text: older.text + prev.text })
      }
      setError('')
    } catch (e) { setError(e instanceof Error ? e.message : String(e)) }
    finally { setLoading(false) }
  }
  return <div className="border-t border-border-subtle">
    <div className="flex items-center gap-2 px-3 py-1.5 text-xs text-fg-tertiary">
      <span>{job.pty ? 'Stored screen · open terminal for scrollback' : running ? 'Output · following unless scrolled up' : 'Output'}</span>
      <div className="ml-auto flex gap-1"><Button size="xs" variant="outline" aria-pressed={wrap} onClick={() => setWrap(!wrap)}>Wrap</Button><Button size="xs" variant="outline" onClick={() => void copy(ansiToPlainText(text))}>Copy output</Button></div>
    </div>
    {page && page.start > 0 && <button className="w-full border-b border-dashed border-border-subtle py-1 text-xs text-project-lapis" disabled={loading} onClick={() => void earlier()}>{loading ? 'Loading…' : `Load earlier output (${jobBytes(page.start)})`}</button>}
    {error && <p role="alert" className="px-3 py-1 text-xs text-danger">{error}</p>}
    {/* ANSI's named colours use theme tokens. Truecolour escapes inherit ink rather than introduce off-palette RGB. */}
    <pre ref={pane} aria-label={`Output of ${job.id}`} onScroll={(e) => { const el = e.currentTarget; follow.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24 }} className={cn('m-0 max-h-60 overflow-auto bg-layer-2 px-3 py-2 font-mono text-xs leading-5 [&_span[style*="color:"]]:text-inherit!', wrap ? 'wrap-anywhere whitespace-pre-wrap' : 'whitespace-pre')}><AnsiText>{text || '(no output)'}</AnsiText></pre>
  </div>
}

/** The sheet card and the expanded transcript tool share this exact body. */
export function JobCard({ job, bodyOnly = false, onJump }: { job: JobView; bodyOnly?: boolean; onJump?(): void }) {
  const api = useApi()
  const [open, setOpen] = useDisclosure(disclosureKey('job', job.session, job.id, 'output'))
  const [terminal, setTerminal] = useDisclosure(disclosureKey('job', job.session, job.id, 'terminal'))
  const [now, setNow] = useState(Date.now)
  const [killing, setKilling] = useState(false)
  const running = job.state === 'running'
  const entries = useSessions((s) => s.sessions[job.session]?.entries ?? EMPTY)
  const origin = useMemo(() => {
    const index = entries.findIndex((e) => e.kind === 'tool' && e.callId === job.call_id)
    const entry = entries[index]
    if (!entry || entry.kind !== 'tool') return null
    const turn = entries.slice(0, index + 1).findLast((e) => e.kind === 'user' && e.turn !== undefined)
    return { entry, turn: turn?.kind === 'user' ? turn.turn : undefined }
  }, [entries, job.call_id])
  useEffect(() => {
    if (!running) return
    const timer = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(timer)
  }, [running])
  const secs = running && job.started_ms != null ? Math.max(0, Math.floor((now - job.started_ms) / 1000)) : job.secs
  const kill = async () => {
    setKilling(true)
    try { await api.cancel_job(job.session, job.id) } catch (e) { toast.error(String(e)) }
    finally { setKilling(false) }
  }
  const jump = () => {
    if (!origin) return
    useToolRuns.getState().setDisclosed(toolKey(origin.entry.callId), true)
    onJump?.()
    useTranscriptJump.getState().request(job.session, origin.entry.id)
  }
  const expanded = bodyOnly || open
  return <section aria-label={`Job ${job.id}`} className={cn('min-w-0 overflow-hidden rounded-xl border border-border-subtle bg-layer-1', bodyOnly && 'mt-2')}>
    <div className="grid grid-cols-[1fr_auto] items-center gap-x-2 gap-y-2 px-3 py-2">
      <div className="flex min-w-0 items-center gap-2">
        {!bodyOnly && <button aria-label={`${open ? 'Collapse' : 'Expand'} ${job.id}`} aria-expanded={open} onClick={() => setOpen(!open)} className="flex items-center gap-2"><ChevronRight className={cn('size-3.5 text-fg-tertiary', open && 'rotate-90')} /><JobState job={job} /></button>}
        {bodyOnly && <JobState job={job} />}<span className="font-mono text-xs text-fg-secondary">{job.id}</span>
      </div>
      <div className="flex items-center gap-2 text-xs text-fg-tertiary"><span className="tabular-nums">{duration(secs)}</span>{running && <Button size="xs" variant="outline" className="text-danger" disabled={killing} onClick={() => void kill()}>Kill</Button>}{job.pty && <><Button size="xs" variant="outline" onClick={() => setTerminal(true)}>Open terminal</Button><PopoutTerminalButton session={job.session} job={job.id} className="size-6" /></>}</div>
      <div className="col-span-2"><Command command={job.command} /></div>
      <div className="col-span-2 flex flex-wrap gap-x-3 gap-y-1 text-[11px] text-fg-tertiary">
        <span>started <b className="font-normal text-fg-secondary">{clock(job.started_ms)}</b></span>{!running && <span>ended {clock(job.ended_ms)}</span>}<span>{job.kind ?? (job.pty ? 'pty' : 'shell')}</span>
        {job.call_id && <button disabled={!origin} className="text-project-lapis enabled:hover:underline disabled:text-fg-tertiary" onClick={jump}>from {origin?.entry.name ?? job.kind ?? 'tool'}{origin?.turn !== undefined && ` · turn ${origin.turn}`}</button>}
        {job.log_bytes != null && <span>log {jobBytes(job.log_bytes)}</span>}
      </div>
      {!expanded && <div className="col-span-2 truncate font-mono text-xs text-fg-tertiary">{ansiToPlainText(job.tail).trimEnd().split('\n').at(-1) || '(no output)'}</div>}
    </div>
    {expanded && <Output key={`${job.session}/${job.id}`} job={job} />}
    {terminal && <Dialog open={terminal} onOpenChange={setTerminal}><DialogContent className="flex h-[75dvh] max-w-[95vw] flex-col sm:max-w-5xl"><DialogHeader><DialogTitle>Terminal · {job.id}</DialogTitle><DialogDescription className="truncate">{job.name ?? job.command}</DialogDescription></DialogHeader><Terminal session={job.session} job={job.id} label={job.name ?? job.command} /></DialogContent></Dialog>}
  </section>
}

export function TranscriptJob({ tool, session }: { tool: Extract<Entry, { kind: 'tool' }>; session: string }) {
  const api = useApi()
  const supported = useQuerySupport(['job_history'])
  const job = tool.job!
  const { data: known } = useQuery({
    queryKey: [...queryKeys.jobs(session), 'detail', job.id],
    enabled: supported,
    queryFn: async ({ signal }) => {
      let before: number | null = null
      while (!signal.aborted) {
        const page = await api.job_history(session, before, 200)
        const found = page.find((j) => j.id === job.id)
        if (found || page.length < 200) return found ?? null
        const next = page.at(-1)?.started_ms
        if (next == null || next === before) return null
        before = next
      }
      return null
    },
  })
  const exit = job.exit?.exit
  const state = exit === undefined ? 'running' : typeof exit === 'string' ? exit : `exited ${exit.exited}`
  const started = tool.startedAt ?? tool.at
  const view: JobView = { ...known, session, id: job.id, command: job.command, state: exit === undefined ? known?.state ?? state : state, secs: job.exit?.secs ?? known?.secs ?? 0, log: known?.log ?? '', tail: job.screen?.rows.join('\n') ?? rows(job.lines).join('\n'), name: known?.name ?? null, pty: known?.pty ?? !!job.screen, call_id: tool.callId, started_ms: known?.started_ms ?? started, ended_ms: known?.ended_ms, kind: known?.kind ?? tool.name }
  return <JobCard job={view} bodyOnly />
}
