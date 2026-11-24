import { useNavigate } from '@tanstack/react-router'
import { AppWindow, ChevronDown, PanelRight, ScrollText, SquareTerminal } from 'lucide-react'
import { useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { type JobRef, openJobLogTile, openJobTerminalTile } from '@/components/tiles/open'
import { useTileHost } from '@/components/tiles/host'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { canPopout, openPopout } from '@/lib/popout'
import { cn } from '@/lib/utils'
import type { Entry } from '@/store/sessions'
import { ToolSpinner } from './Indicators'
import { LiveJob } from './JobsSheet'

type Tool = Extract<Entry, { kind: 'tool' }>
/** `pty`: an interactive terminal (`start()` or spawned); else a `shell()` background job, shown as its log. */
export type PtyCall = { title: string; job: JobRef | null; pty: boolean; finished: boolean }

/** The receipt's first line, one of two forms (211 §1, §5): `pty view opened · <title> · tile pty:<session>/<job>` for
 * a PTY, `job view opened · <title> · tile job:<session>/<job>` for a background job. The transcript keeps a call's
 * content, not its `structured` value, so the job comes from here; the arguments stand in while pending. */
const PTY_RECEIPT = /^pty view opened · (.*) · tile pty:([^/\s]+)\/(\S+)$/m
const JOB_RECEIPT = /^job view opened · (.*) · tile job:([^/\s]+)\/(\S+)$/m

export function ptyCall(tool: Pick<Tool, 'args' | 'result'>): PtyCall {
  const output = tool.result && !tool.result.isError ? tool.result.output : ''
  const pty = PTY_RECEIPT.exec(output)
  const m = pty ?? JOB_RECEIPT.exec(output)
  if (m) {
    return { title: m[1] as string, job: { session: m[2] as string, job: m[3] as string }, pty: pty !== null, finished: output.includes('The job has finished') }
  }
  const args = (tool.args ?? {}) as { job?: unknown; command?: unknown; name?: unknown; title?: unknown }
  const title = [args.title, args.job, args.name, args.command].find((a): a is string => typeof a === 'string' && a !== '') ?? ''
  return { title, job: null, pty: true, finished: false }
}

const popOut = (job: JobRef) => void openPopout({ kind: 'pty', ...job }).catch((e: unknown) => toast.error(String(e)))

/**
 * The transcript card of a `show_pty` call (211 §3, §5): title, job, **Open** beside the chat in leaf `host`. A PTY
 * opens its terminal tile; with no tile there its own window, or on iOS the `/pty` page; pop out where windows exist.
 * A background job opens its log tile; with no tile there (a phone) the card unfolds the live log itself.
 */
export function PtyViewCard({ tool }: { tool: Tool }) {
  const host = useTileHost()
  const navigate = useNavigate()
  const [log, setLog] = useState(false)
  const call = ptyCall(tool)
  const failed = tool.result?.isError === true
  const running = !tool.interrupted && (tool.pending || !tool.result)
  const job = call.job
  const open = (j: JobRef) => {
    if (!call.pty) {
      if (!host || openJobLogTile(host, j, call.title) === null) setLog((l) => !l)
      return
    }
    if (host && openJobTerminalTile(host, j, call.title) !== null) return
    if (canPopout()) popOut(j)
    else void navigate({ to: '/pty/$session/$job', params: j })
  }
  const Icon = call.pty ? SquareTerminal : ScrollText
  return (
    <div className="my-2 min-w-0 rounded-xl border border-border-subtle bg-layer-1 px-4 py-3 text-sm" data-pty-card={job ? `${job.session}/${job.job}` : undefined}>
      <div className="flex min-w-0 items-center gap-3">
        <Icon className="size-5 shrink-0 text-fg-secondary" />
        <div className="flex min-w-0 flex-1 flex-col">
          <span className="truncate font-medium">{call.title || 'Terminal'}</span>
          {job && <span className="truncate font-mono text-xs text-fg-secondary">{job.job}{!call.pty && ' · log'}{call.finished && ' · finished'}</span>}
        </div>
        {running && <ToolSpinner />}
        {job && (
          <Button variant="outline" size="sm" aria-expanded={call.pty ? undefined : log} onClick={() => open(job)}>
            {host || call.pty ? <PanelRight /> : <ChevronDown className={cn(log && 'rotate-180')} />}
            Open
          </Button>
        )}
        {job && call.pty && canPopout() && (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button variant="ghost" size="icon-sm" className="text-fg-secondary" aria-label="Open in new window" onClick={() => popOut(job)}>
                <AppWindow className="size-4" />
              </Button>
            </TooltipTrigger>
            <TooltipContent>Open in new window</TooltipContent>
          </Tooltip>
        )}
      </div>
      {job && log && <LiveJob session={job.session} job={job.job} className="mt-2 max-h-96" />}
      {failed && <p className="mt-2 text-xs text-destructive wrap-anywhere">{tool.result?.output}</p>}
    </div>
  )
}
