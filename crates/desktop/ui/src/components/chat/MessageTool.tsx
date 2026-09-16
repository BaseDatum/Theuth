import { Loader2 } from 'lucide-react'
import { memo, useMemo, useState } from 'react'
import { Streamdown } from 'streamdown'
import { codeDetails, describe, resultLine } from '@/lib/describe'
import { type DiffLine, diffLines } from '@/lib/diff'
import { rows } from '@/lib/jobTail'
import { cn } from '@/lib/utils'
import { type Entry, type JobTail, useSessions } from '@/store/sessions'
import { ArtifactCard, Progress } from './ArtifactCard'
import { Attachments } from './Attachments'
import { fenced, plugins, shikiTheme } from './MessageAssistant'

type Tool = Extract<Entry, { kind: 'tool' }>

function Job({ job }: { job: JobTail }) {
  const lines = job.screen ? job.screen.rows : rows(job.lines)
  const exit = job.exit && `exit ${typeof job.exit.exit === 'string' ? job.exit.exit : job.exit.exit.exited} · ${job.exit.secs}s`
  return (
    <div className="mt-2 font-mono text-xs leading-5 text-fg-tertiary">
      <div className="italic">
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

/** `⚙ name` row with the described arguments; ✓/✗ + first result line when done, click shows the whole output. A `show` call is an artifact card. */
export const MessageTool = memo(function MessageTool({ tool, session }: { tool: Tool; session: string }) {
  if (tool.name === 'show') return <ArtifactCard tool={tool} session={session} />
  return <PlainTool tool={tool} session={session} />
})

function PlainTool({ tool, session }: { tool: Tool; session: string }) {
  const [expanded, setExpanded] = useState(false)
  const source = useMemo(() => codeDetails(tool.name, tool.args), [tool.name, tool.args])
  const lines = useMemo(() => describe(tool.name, source?.args ?? tool.args), [tool.name, tool.args, source])
  const progress = useSessions((s) => (tool.pending ? s.sessions[session]?.progress.get(tool.callId) : undefined))
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
          <span className="shrink-0 text-fg-secondary">{tool.child ? '↳' : '⚙'}</span>
          <span className="wrap-anywhere">{tool.name}</span>
          {running && <Loader2 className="size-3.5 shrink-0 animate-spin text-fg-tertiary" />}
        </div>
        {lines.length > 0 && (
          <pre className="w-full font-mono text-xs leading-5 wrap-anywhere whitespace-pre-wrap text-fg-secondary">
            {lines.join('\n')}
          </pre>
        )}
        {progress && <Progress tool={tool.name} progress={progress} />}
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
      {result && result.files.length > 0 && <Attachments files={result.files} className="mt-2" />}
      {expanded && tool.job && <Job job={tool.job} />}
    </div>
  )
}
