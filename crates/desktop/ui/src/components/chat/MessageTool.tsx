import { Timer, Wrench } from 'lucide-react'
import { memo, useEffect, useMemo, useState } from 'react'
import { Streamdown } from 'streamdown'
import { ToolSpinner } from './Indicators'
import { codeDetails, describe, resultLine } from '@/lib/describe'
import { type DiffLine, diffLines } from '@/lib/diff'
import { rows } from '@/lib/jobTail'
import { cn } from '@/lib/utils'
import { type Entry, type JobTail, useSessions } from '@/store/sessions'
import { ArtifactCard, Progress } from './ArtifactCard'
import { AnsweredAskCard, askQuestions } from './AskCard'
import { BoardCard, isBoardCreate } from './BoardCard'
import { ScrapeCard } from './ScrapeCard'
import { Attachments } from './Attachments'
import { fenced, plugins, shikiTheme } from './MessageAssistant'

type Tool = Extract<Entry, { kind: 'tool' }>

function Job({ job }: { job: JobTail }) {
  const lines = job.screen ? job.screen.rows : rows(job.lines)
  // 109 §8.4: `lost` = the job died with the engine (never an exit code).
  const lost = job.exit?.exit === 'lost'
  const exit = job.exit && `${lost ? 'lost with the engine' : `exit ${typeof job.exit.exit === 'string' ? job.exit.exit : job.exit.exit.exited}`} · ${job.exit.secs}s`
  return (
    <div className="mt-2 font-mono text-xs leading-5 text-fg-tertiary">
      <div className={lost ? 'italic text-warning' : 'italic'}>
        {job.id}
        {exit && ` · ${exit}`}
      </div>
      <div className="max-h-[12.5rem] overflow-y-auto">
        {lines.map((l, i) => (
          // Positional rows of a scrolling tail: the index is the identity.
          <div key={i} className="truncate text-fg-secondary">
            {l}
          </div>
        ))}
      </div>
    </div>
  )
}

/** `old`/`new` of an `edit` call when the arguments carry both. */
function editArgs(args: unknown): { old: string; new: string } | null {
  if (typeof args !== 'object' || args === null) return null
  const { old, new: next } = args as Record<string, unknown>
  return typeof old === 'string' && typeof next === 'string' ? { old, new: next } : null
}

/** Source shown only inside an expanded tool card. */
function Source({ code, language }: { code: string; language: string }) {
  return (
    <Streamdown
      mode="static"
      plugins={plugins}
      shikiTheme={shikiTheme}
      className="mt-2 text-xs [&_[data-streamdown=code-block]]:my-0 [&_pre]:max-h-96 [&_pre]:overflow-auto"
    >
      {fenced(code, language)}
    </Streamdown>
  )
}

const ROW: Record<DiffLine['op'], string> = {
  ' ': 'text-fg-tertiary',
  '-': 'bg-danger-bg text-danger',
  '+': 'bg-success-bg text-success',
}

/** Unified line diff of an `edit`'s `old` → `new`. */
function Diff({ old, new: next }: { old: string; new: string }) {
  const lines = useMemo(() => diffLines(old, next), [old, next])
  return (
    <pre className="mt-2 max-h-96 overflow-auto rounded-lg bg-layer-2 py-2 font-mono text-xs leading-5">
      {lines.map((l, i) => (
        // Positional rows of one diff: the index is the identity.
        <div key={i} className={cn('px-3 whitespace-pre-wrap wrap-anywhere', ROW[l.op])}>
          {l.op} {l.text}
        </div>
      ))}
    </pre>
  )
}

/** `mm:ss` under an hour, `h:mm` from there. */
function countdown(ms: number): string {
  const s = Math.max(0, Math.ceil(ms / 1000))
  const pad = (n: number) => String(n).padStart(2, '0')
  return s >= 3600 ? `${Math.floor(s / 3600)}:${pad(Math.floor((s % 3600) / 60))}` : `${pad(Math.floor(s / 60))}:${pad(s % 60)}`
}

/** `remind_me_in` (14 as-built): the message and a countdown to `fires_at` (parsed from the result line), ticking each
 * second; `fired` at zero, `delivered` once the `<reminder>` card with the same id is in the transcript. */
function ReminderCard({ tool, session }: { tool: Tool; session: string }) {
  const message = typeof tool.args === 'object' && tool.args !== null ? String((tool.args as Record<string, unknown>).message ?? '') : ''
  const output = tool.result?.output ?? ''
  const id = /^(r\d+) set/.exec(output)?.[1]
  const firesAt = useMemo(() => {
    const t = /fires at (\S+)/.exec(output)?.[1]
    const ms = t === undefined ? Number.NaN : new Date(t).getTime()
    return Number.isNaN(ms) ? undefined : ms
  }, [output])
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (firesAt === undefined || firesAt <= now) return
    const t = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(t)
  }, [firesAt, now])
  const delivered = useSessions((s) => id !== undefined && !!s.sessions[session]?.entries.some((e) => e.kind === 'peer' && e.from.startsWith(`Reminder ${id} `)))
  const failed = tool.result?.isError
  const state = failed ? undefined : delivered ? 'delivered' : firesAt === undefined ? undefined : firesAt > now ? countdown(firesAt - now) : 'fired'
  return (
    <div className="my-2 min-w-0 rounded-xl border border-border-subtle bg-layer-1 px-4 py-3 text-sm">
      <div className="flex items-center gap-2 font-medium">
        <Timer className="size-3.5 shrink-0 text-fg-secondary" />
        <span className="wrap-anywhere">Reminder{id ? ` ${id}` : ''}</span>
        {(tool.pending || !tool.result) && <ToolSpinner />}
        {state && <span className={cn('ml-auto shrink-0 font-mono text-xs tabular-nums', state === 'delivered' ? 'text-success' : 'text-fg-tertiary')}>{state}</span>}
      </div>
      {message && <div className="mt-1 wrap-anywhere whitespace-pre-wrap text-fg-secondary">{message}</div>}
      {failed && <div className="mt-1 truncate font-mono text-xs text-danger">✗ {resultLine(output)}</div>}
    </div>
  )
}

/** Wrench + `name` row with the described arguments; ✓/✗ + first result line when done, click shows the whole output. A `show` or `app` call is an artifact card; `crawl`/`map_site`/`scrape_url`/`search_web` show stacked site favicons (search lists its sources, read ones first).
 * Specialized cards draw their own previews; the stored, model-visible occurrences (48 §2.3) render beneath them with their removal controls, since an artifact on screen is not necessarily in the frame and removing an occurrence never deletes the artifact.
 * `readOnly` (an inspection page) shows the occurrences without removal controls. */
export const MessageTool = memo(function MessageTool({ tool, session, readOnly }: { tool: Tool; session: string; readOnly?: boolean }) {
  // An answered `ask_user` keeps its questions and the sent answers on screen (`AskCard` while it is pending).
  const asked = tool.name === 'ask_user' && tool.result && !tool.result.isError ? askQuestions(tool.args) : null
  if (asked && tool.result) return <AnsweredAskCard questions={asked} output={tool.result.output} session={session} callId={tool.callId} />
  const card =
    tool.name === 'show' || tool.name === 'app' || tool.name === 'task_draft' || tool.name === 'agent_draft' || tool.name === 'routine_draft' ? (
      <ArtifactCard tool={tool} session={session} />
    ) : tool.name === 'crawl' || tool.name === 'map_site' || tool.name === 'scrape_url' || tool.name === 'search_web' ? (
      <ScrapeCard tool={tool} session={session} />
    ) : tool.name === 'remind_me_in' ? (
      <ReminderCard tool={tool} session={session} />
    ) : isBoardCreate(tool) ? (
      <BoardCard tool={tool} session={session} readOnly={readOnly} />
    ) : null
  if (!card) return <PlainTool tool={tool} session={session} readOnly={readOnly} />
  return (
    <>
      {card}
      <Attachments occurrences={tool.context?.attachments} session={readOnly ? undefined : session} active={tool.context?.active} className="-mt-1 mb-2" />
    </>
  )
})

function PlainTool({ tool, session, readOnly }: { tool: Tool; session: string; readOnly?: boolean }) {
  const [expanded, setExpanded] = useState(false)
  const source = useMemo(() => codeDetails(tool.name, tool.args), [tool.name, tool.args])
  const lines = useMemo(() => describe(tool.name, source?.args ?? tool.args), [tool.name, tool.args, source])
  const progress = useSessions((s) => (tool.pending ? s.sessions[session]?.progress.get(tool.callId) : undefined))
  const sandbox = useSessions((s) => (tool.pending ? s.sessions[session]?.sandbox.get(tool.callId) : undefined))
  const { result } = tool
  const running = tool.pending || !result
  const edit = tool.name === 'edit' ? editArgs(tool.args) : null
  const expandable = !!(source || edit || result || tool.job)
  return (
    <div className="my-2 min-w-0 rounded-xl border border-border-subtle bg-layer-1 px-4 py-3 text-sm">
      <button
        type="button"
        className="flex w-full min-w-0 flex-col gap-1 text-left"
        aria-expanded={expandable ? expanded : undefined}
        onClick={() => expandable && setExpanded((v) => !v)}
      >
        <div className="flex items-center gap-2 font-medium">
          {tool.child ? <span className="shrink-0 text-fg-secondary">↳</span> : <Wrench className="size-3.5 shrink-0 text-fg-secondary" />}
          <span className="wrap-anywhere">{tool.name}</span>
          {running && <ToolSpinner />}
        </div>
        {lines.length > 0 && (
          <pre className="w-full font-mono text-xs leading-5 wrap-anywhere whitespace-pre-wrap text-fg-secondary">
            {lines.join('\n')}
          </pre>
        )}
        {progress && <Progress tool={tool.name} progress={progress} />}
        {sandbox && (
          <div className="w-full truncate font-mono text-xs text-fg-secondary">
            {sandbox.step} · {sandbox.line}
            {sandbox.pct !== null && ` · ${sandbox.pct}%`}
          </div>
        )}
        {result && (
          <div className={cn('w-full truncate font-mono text-xs', result.isError ? 'text-danger' : 'text-success')}>
            {result.isError ? '✗' : '✓'} {resultLine(result.output)}
          </div>
        )}
      </button>
      {expanded && source && <Source code={source.code} language={source.language} />}
      {expanded && edit && <Diff old={edit.old} new={edit.new} />}
      {expanded && result && (
        <pre className="mt-2 max-h-56 overflow-auto rounded-lg bg-layer-2 p-3 font-mono text-xs leading-5 wrap-anywhere whitespace-pre-wrap">
          {result.output}
        </pre>
      )}
      {result && <Attachments files={result.files} occurrences={tool.context?.attachments} session={readOnly ? undefined : session} active={tool.context?.active} className="mt-2" />}
      {expanded && tool.job && <Job job={tool.job} />}
    </div>
  )
}
