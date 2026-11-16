import { Link } from '@tanstack/react-router'
import { Fragment, type ReactNode, useMemo } from 'react'
import { toast } from 'sonner'
import { dotLabel } from '@/components/agents/sessions'
import { useSessionDrag } from '@/components/chat/Panes'
import { canPopout, openPopout } from '@/lib/popout'
import { backgroundColors } from '@/lib/background-colors'
import { STRIP_CAP, stripRows } from '@/lib/session-tree'
import { cn } from '@/lib/utils'
import type { AgentInfo } from '@/store/ui-events'
import { useScope } from '@/store/engine-scope'
import { useSessionField } from '@/store/sessions'

/** One row of category dots, independently capped; the Advisor pill remains last. */
export function SubagentStrip({ session, trailing }: { session: string; trailing?: ReactNode }) {
  const { sessions: store } = useScope()
  const children = useSessionField(session, 'children')
  const { shown, more } = useMemo(() => stripRows(children ?? {}), [children])
  const jobs = useSessionField(session, 'jobs')
  const ptys = useSessionField(session, 'ptys')
  const terminals = useSessionField(session, 'terminals')
  const terminalIds = useMemo(() => new Set([...(terminals ?? []), ...(ptys ?? [])]), [terminals, ptys])
  const reminders = useSessionField(session, 'reminders')
  const details = useSessionField(session, 'reminderDetails')
  const beats = useSessionField(session, 'jobBeats')
  const reveal = (card: 'jobs' | 'reminders') => {
    store.getState().setTodosShown(session, true)
    store.getState().setBackgroundExpanded(session, card, true)
  }
  const groups: ReactNode[] = []
  if (shown.length) groups.push(
    <div className="flex items-center" aria-label={`${shown.length + more} subagents running`}>
      {shown.map((row) => <SubagentLink key={row.id} row={row} className="subagent-dot-in flex size-6 shrink-0 items-center justify-center"><ActivityDot color={backgroundColors[row.state.state === 'approval' ? 'approval' : 'subagents'].bg} beat={row.beat} /></SubagentLink>)}
      {more > 0 && <span className="shrink-0 tabular-nums">+{more}</span>}
    </div>,
  )
  for (const [category, ids, color] of [
    ['Jobs', jobs, backgroundColors.jobs.bg],
    ['Terminals', terminalIds, backgroundColors.terminals.bg],
    ['Reminders', reminders, backgroundColors.reminders.bg],
  ] as const) {
    if (!ids?.size) continue
    groups.push(
      <div className="flex items-center" aria-label={`${ids.size} ${category.toLowerCase()}`}>
        {[...ids].slice(0, STRIP_CAP).map((id) => {
          const label = category === 'Reminders' ? `Reminder ${id} · ${details?.get(id)?.message ?? 'pending'}` : `${category === 'Jobs' ? 'Job' : 'Terminal'} ${id} · ${category === 'Jobs' ? 'show running jobs' : 'open terminal'}`
          const dot = <ActivityDot color={color} beat={category === 'Reminders' ? undefined : beats?.[id]} />
          return category === 'Terminals'
            ? <TerminalLink key={id} session={session} job={id} label={label} className="flex size-6 shrink-0 items-center justify-center">{dot}</TerminalLink>
            : <button key={id} type="button" title={label} aria-label={label} className="flex size-6 shrink-0 items-center justify-center rounded outline-none focus-visible:ring-2 focus-visible:ring-ring" onClick={() => reveal(category === 'Jobs' ? 'jobs' : 'reminders')}>{dot}</button>
        })}
        {ids.size > STRIP_CAP && <span className="shrink-0 tabular-nums">+{ids.size - STRIP_CAP}</span>}
      </div>,
    )
  }
  if (!groups.length && !trailing) return null
  return (
    <div className="mx-auto flex min-h-7 w-full shrink-0 select-none items-center justify-center gap-3 px-4 text-xs text-fg-tertiary md:max-w-[768px]">
      {groups.length > 0 && <div className="flex min-w-0 items-center gap-2 overflow-x-auto">
        {groups.map((group, i) => <Fragment key={i}>{i > 0 && <span role="separator" className="h-3 w-px shrink-0 bg-border" />}{group}</Fragment>)}
      </div>}
      {trailing}
    </div>
  )
}

export function ActivityDot({ color, beat }: { color: string; beat?: number }) {
  return <span key={beat ?? 0} className={cn('subagent-dot size-2 shrink-0 rounded-full', color, beat !== undefined && 'subagent-blink')} />
}

/** Identical drag target for a subagent's dot and detail row. */
export function SubagentLink({ row, children, className }: { row: AgentInfo; children: ReactNode; className: string }) {
  const drag = useSessionDrag(row.id, row.name)
  const label = dotLabel(row)
  return <Link to="/c/$session" params={{ session: row.id }} {...drag} title={label} aria-label={label} className={className}>{children}</Link>
}

/** Same popout as PopoutTerminalButton, with an in-page terminal on iOS. */
export function TerminalLink({ session, job, label, className, children }: { session: string; job: string; label: string; className: string; children: ReactNode }) {
  return (
    <Link to="/pty/$session/$job" params={{ session, job }} title={label} aria-label={label} className={className} onClick={(e) => {
      if (!canPopout() || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return
      e.preventDefault()
      void openPopout({ kind: 'pty', session, job }).catch((e: unknown) => toast.error(String(e)))
    }}>{children}</Link>
  )
}
