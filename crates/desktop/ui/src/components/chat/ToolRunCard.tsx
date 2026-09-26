import { ChevronRight, Loader2 } from 'lucide-react'
import { resultLine } from '@/lib/describe'
import { useToolRuns } from '@/lib/tool-runs'
import { cn } from '@/lib/utils'
import type { Prompt } from '@/store/approvals'
import type { Entry } from '@/store/sessions'
import { Message, PromptCard } from './Message'

type Tool = Extract<Entry, { kind: 'tool' }>

/** A run: tool rows, possibly with reasoning rows between them; starts and ends with a tool. */
export type Run = Entry[]

/** Entries with every maximal run of two or more tool cards (reasoning between them rides along; any other row,
 * including assistant text, ends the run) folded into one `Run`. A lone tool call stays a plain entry. */
export function foldRuns(entries: Entry[]): (Entry | Run)[] {
  const out: (Entry | Run)[] = []
  let i = 0
  while (i < entries.length) {
    const e = entries[i] as Entry
    if (e.kind !== 'tool') {
      out.push(e)
      i++
      continue
    }
    let end = i + 1
    let lastTool = i
    while (end < entries.length && (entries[end]?.kind === 'tool' || entries[end]?.kind === 'reasoning')) {
      if (entries[end]?.kind === 'tool') lastTool = end
      end++
    }
    const run = entries.slice(i, lastTool + 1)
    if (run.length > 1) out.push(run)
    else out.push(e)
    i = lastTool + 1
  }
  return out
}

export type Summary = {
  /** Header line: `running \`name\` · 3 of 5` live, `5 tools · 2.1 s · 1 failed (name)` once every call has a result. */
  text: string
  running: boolean
  failed: boolean
  /** The last completed call's `✓/✗ first line`, when any call completed. */
  hint: string | null
}

export function summarize(run: Run): Summary {
  const tools = run.filter((e): e is Tool => e.kind === 'tool')
  const current = tools.findIndex((t) => !t.result)
  const done = tools.filter((t) => t.result)
  const failed = done.filter((t) => t.result?.isError)
  const last = done[done.length - 1]
  const hint = last?.result ? `${last.result.isError ? '✗' : '✓'} ${resultLine(last.result.output)}` : null
  if (current >= 0) {
    return { text: `running \`${tools[current]?.name}\` · ${current + 1} of ${tools.length}`, running: true, failed: failed.length > 0, hint }
  }
  const parts = [`${tools.length} tool${tools.length === 1 ? '' : 's'}`]
  const start = tools[0]?.startedAt
  const end = tools[tools.length - 1]?.finishedAt
  // `2.1 s` under a minute, `3 min` from there.
  if (start !== undefined && end !== undefined) parts.push(end - start < 60_000 ? `${((end - start) / 1000).toFixed(1)} s` : `${Math.round((end - start) / 60_000)} min`)
  if (failed.length) parts.push(`${failed.length} failed (${failed.map((t) => t.name).join(', ')})`)
  return { text: parts.join(' · '), running: false, failed: failed.length > 0, hint }
}

/** One card for a run of consecutive tool calls (70 TOOL-COLLAPSE): collapsed by default to a live status line, expanded
 * to the same cards the transcript shows ungrouped. Open state lives in `useToolRuns.open` keyed by the first call id, so it
 * survives the settled reload (new entry ids, same call ids). Prompts waiting on a call in the run render under the header
 * whether or not it is open: an approval must never hide inside a collapsed run. */
export function ToolRunCard({ run, prompts, session }: { run: Run; prompts: Prompt[] | null; session: string }) {
  const first = run[0] as Tool
  const open = useToolRuns((s) => s.open[first.callId] ?? false)
  const toggle = useToolRuns((s) => s.toggle)
  const { text, running, failed, hint } = summarize(run)
  return (
    <>
      <div className={cn('my-2 min-w-0 rounded-xl border bg-layer-1 px-4 py-3 text-sm', failed ? 'border-danger-border' : 'border-border-subtle')}>
        <button type="button" className="flex w-full min-w-0 flex-col gap-1 text-left" aria-expanded={open} onClick={() => toggle(first.callId)}>
          <div className={cn('flex items-center gap-2 font-medium', failed && 'text-danger')}>
            <ChevronRight className={cn('size-4 shrink-0 transition-transform', open && 'rotate-90')} />
            <span className="shrink-0 text-fg-secondary">⚙</span>
            <span className="min-w-0 truncate">{text}</span>
            {running && <Loader2 className="size-3.5 shrink-0 animate-spin text-fg-tertiary" />}
          </div>
          {hint && !open && (
            <div className={cn('w-full truncate pl-6 font-mono text-xs', hint.startsWith('✗') ? 'text-danger' : 'text-success')}>{hint}</div>
          )}
        </button>
      </div>
      {open && run.map((e) => <Message key={e.id} entry={e} prompts={null} session={session} />)}
      {prompts?.map((p) => <PromptCard key={p.id} prompt={p} />)}
    </>
  )
}
