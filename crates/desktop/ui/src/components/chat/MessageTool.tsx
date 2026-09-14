import { Loader2 } from 'lucide-react'
import { memo, useMemo, useState } from 'react'
import { describe, resultLine } from '@/lib/describe'
import { rows } from '@/lib/jobTail'
import { cn } from '@/lib/utils'
import type { Entry, JobTail } from '@/store/sessions'
import { Attachments } from './Attachments'

type Tool = Extract<Entry, { kind: 'tool' }>

function Job({ job }: { job: JobTail }) {
  const lines = job.screen ? job.screen.rows : rows(job.lines)
  const exit = job.exit && `exit ${typeof job.exit.exit === 'string' ? job.exit.exit : job.exit.exit.exited} · ${job.exit.secs}s`
  return (
    <div className="mt-2 font-mono text-xs leading-5 text-fg-tertiary">
      <div className="italic">
        {job.id} · {job.command}
        {exit && ` · ${exit}`}
      </div>
      {lines.map((l, i) => (
        // Positional rows of a scrolling tail: the index is the identity.
        <div key={i} className="truncate text-fg-secondary">
          {l}
        </div>
      ))}
    </div>
  )
}

/** `⚙ name` row with the described arguments; ✓/✗ + first result line when done, click shows the whole output. */
export const MessageTool = memo(function MessageTool({ tool }: { tool: Tool }) {
  const [expanded, setExpanded] = useState(false)
  const lines = useMemo(() => describe(tool.name, tool.args), [tool.name, tool.args])
  const { result } = tool
  const running = tool.pending || !result
  return (
    <div className="my-2 rounded-xl border border-border-subtle bg-layer-1 px-4 py-3 text-sm">
      <button
        type="button"
        className="flex w-full flex-col items-start gap-1 text-left"
        onClick={() => result && setExpanded((v) => !v)}
      >
        <div className="flex items-center gap-2 font-medium">
          <span className="text-fg-secondary">{tool.child ? '↳' : '⚙'}</span>
          {tool.name}
          {running && <Loader2 className="size-3.5 animate-spin text-fg-tertiary" />}
        </div>
        {lines.length > 0 && (
          <pre className="max-w-full overflow-x-auto font-mono text-xs leading-5 text-fg-secondary whitespace-pre-wrap">
            {lines.join('\n')}
          </pre>
        )}
        {result && (
          <div className={cn('truncate font-mono text-xs', result.isError ? 'text-danger' : 'text-success')}>
            {result.isError ? '✗' : '✓'} {resultLine(result.output)}
          </div>
        )}
      </button>
      {expanded && result && (
        <pre className="mt-2 max-h-96 overflow-auto rounded-lg bg-layer-2 p-3 font-mono text-xs leading-5 whitespace-pre-wrap">
          {result.output}
        </pre>
      )}
      {result && result.files.length > 0 && <Attachments files={result.files} className="mt-2" />}
      {tool.job && <Job job={tool.job} />}
    </div>
  )
})
