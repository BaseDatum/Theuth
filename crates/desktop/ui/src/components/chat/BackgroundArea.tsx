import { useQuery } from '@tanstack/react-query'
import { type ReactNode, useEffect, useMemo, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { liveRows, modelLabel } from '@/components/agents/sessions'
import { backgroundColors } from '@/lib/background-colors'
import { usePanes } from '@/lib/panes'
import { fmtThought } from '@/lib/time'
import { queryKeys, useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'
import type { PendingReminderView } from '@/rpc/bindings/method.agent.output'
import { useScope } from '@/store/engine-scope'
import { useSessionField } from '@/store/sessions'
import { BackgroundCard } from './BackgroundCard'
import { ActivityDot, BackgroundDashes, SubagentDashes, SubagentLink, TerminalLink } from './SubagentStrip'
import { openSurface, surfaceOf } from './slash'
import { TodoCard } from './TodoCard'

function useClock(enabled: boolean) {
  const [now, setNow] = useState(Date.now)
  useEffect(() => {
    if (!enabled) return
    setNow(Date.now())
    const timer = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(timer)
  }, [enabled])
  return now
}

export function countdown(firesAt: number, now: number): string {
  const seconds = Math.max(0, Math.ceil(firesAt - now / 1000))
  if (!seconds) return 'due now'
  const hours = Math.floor(seconds / 3600)
  const minutes = Math.floor(seconds / 60) % 60
  const tail = `${String(minutes).padStart(hours ? 2 : 1, '0')}:${String(seconds % 60).padStart(2, '0')}`
  return hours ? `${hours}:${tail}` : tail
}

/**
 * One grabber, fixed against the composer; every populated category opens above it. The activity dashes grow out of
 * the bar's ends on its row (subagents left, jobs · terminals · reminders right), the `trailing` Advisor pill last.
 */
export function BackgroundArea({ session, trailing }: { session: string; trailing?: ReactNode }) {
  const { sessions: store } = useScope()
  const shown = useSessionField(session, 'todosShown') ?? false
  const todos = useSessionField(session, 'todos')
  const stale = useSessionField(session, 'todosStale')
  const children = useSessionField(session, 'children')
  const agents = useMemo(() => liveRows(children ?? {}), [children])
  const jobs = useSessionField(session, 'jobs')
  const terminals = useSessionField(session, 'terminals')
  const ptys = useSessionField(session, 'ptys')
  const reminders = useSessionField(session, 'reminders')
  const details = useSessionField(session, 'reminderDetails')
  const subagentsExpanded = useSessionField(session, 'subagentsExpanded') ?? false
  const jobsExpanded = useSessionField(session, 'jobsExpanded') ?? false
  const remindersExpanded = useSessionField(session, 'remindersExpanded') ?? false
  const now = useClock(shown)
  const focused = usePanes((s) => s.focused === session)
  useEffect(() => {
    if (!focused) return
    const on = (e: Event) => {
      if (surfaceOf(e) === 'todos') store.getState().setTodosShown(session, !store.getState().sessions[session]?.todosShown)
    }
    window.addEventListener('theuth:open', on)
    return () => window.removeEventListener('theuth:open', on)
  }, [session, store, focused])

  const hasTodos = !!todos && (!!todos.items.length || stale)
  const terminalCount = new Set([...(terminals ?? []), ...(ptys ?? [])]).size
  const jobCount = jobs?.size ?? 0
  const reminderCount = reminders?.size ?? 0
  // `agents` (liveRows) is the uncapped stripRows, so every dash the row could show keeps the bar visible.
  if (!hasTodos && !agents.length && !jobCount && !terminalCount && !reminderCount) {
    return trailing ? <div className="mx-auto flex min-h-7 w-full shrink-0 select-none items-center justify-center px-4 text-xs text-fg-tertiary md:max-w-[768px]">{trailing}</div> : null
  }
  const waiting = agents.filter((a) => a.state.state === 'approval').length
  const pending = [...(reminders ?? [])].map((id) => details?.get(id)).filter((r): r is PendingReminderView => !!r)
  const next = pending.length ? Math.min(...pending.map((r) => r.fires_at)) : null
  const blockId = `background-${session}`
  const label = shown ? 'Hide background activity' : 'Show background activity'
  return (
    <>
      {shown && (
        <div id={blockId} className="mx-auto flex max-h-[calc(var(--visible-band,100dvh)/2)] w-full max-w-[768px] shrink-0 flex-col gap-1.5 overflow-y-auto px-4">
          {hasTodos && <TodoCard session={session} />}
          {!!agents.length && (
            <BackgroundCard title="Subagents" color={backgroundColors.subagents.bg} summary={`${agents.length} live${waiting ? ` · ${waiting} waiting on approval` : ''}`} expanded={subagentsExpanded} onExpanded={(open) => store.getState().setBackgroundExpanded(session, 'subagents', open)}>
              <ul>
                {agents.map((row) => (
                  <li key={row.id}>
                    <SubagentLink row={row} className="flex min-h-8 items-center gap-2 rounded py-1 outline-none hover:bg-layer-2 focus-visible:ring-2 focus-visible:ring-ring">
                      <ActivityDot color={backgroundColors[row.state.state === 'approval' ? 'approval' : 'subagents'].bg} />
                      <span className="min-w-0 flex-1"><span className="block truncate font-medium">{row.name}</span><span className="block truncate text-fg-tertiary">{modelLabel(row)}</span></span>
                      <span className="text-fg-secondary">{row.state.state === 'approval' ? 'waiting on approval' : row.state.state === 'awaiting' ? 'waiting on peer' : 'running'}</span>
                      <span className="shrink-0 text-fg-tertiary tabular-nums">{row.started_at ? fmtThought(Math.max(0, now - Date.parse(row.started_at))) ?? '0s' : '—'}</span>
                    </SubagentLink>
                  </li>
                ))}
              </ul>
            </BackgroundCard>
          )}
          {(jobCount > 0 || terminalCount > 0) && (
            <BackgroundCard title="Jobs" color={backgroundColors.jobs.bg} summary={[jobCount > 0 && `${jobCount} running`, terminalCount > 0 && `${terminalCount} terminal${terminalCount === 1 ? '' : 's'}`].filter(Boolean).join(' · ')} expanded={jobsExpanded} onExpanded={(open) => store.getState().setBackgroundExpanded(session, 'jobs', open)}>
              {jobsExpanded && <JobsRows session={session} />}
            </BackgroundCard>
          )}
          {reminderCount > 0 && (
            <BackgroundCard title="Reminders" color={backgroundColors.reminders.bg} summary={`${reminderCount} pending${next === null ? '' : ` · ${countdown(next, now) === 'due now' ? 'due now' : `in ${countdown(next, now)}`}`}`} expanded={remindersExpanded} onExpanded={(open) => store.getState().setBackgroundExpanded(session, 'reminders', open)}>
              {remindersExpanded && <ul>{[...(reminders ?? [])].map((id) => <ReminderRow key={id} session={session} id={id} reminder={details?.get(id)} now={now} />)}</ul>}
            </BackgroundCard>
          )}
        </div>
      )}
      {/* 1fr auto 1fr keeps the bar centred whatever each side holds. A side that runs out of width wraps upward
          (`flex-wrap-reverse`): the first dashes stay on the bottom row, level with the bar (`items-end`), the rest
          stack above in the same order; nothing overflows the pane sideways. Their -m-3 lets the dashes reach into the
          button's padding so they sit a dash-gap from the bar's ends. */}
      <div data-dash-row="" className="mx-auto grid min-h-6 w-full max-w-[768px] shrink-0 select-none grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] items-end px-4 text-xs text-fg-tertiary">
        <div className="pointer-events-none relative z-10 -mr-3 flex min-w-0 items-end justify-end py-0.5 [&>*]:pointer-events-auto">
          <SubagentDashes session={session} />
        </div>
        <button type="button" aria-expanded={shown} aria-controls={blockId} aria-label={label} title={label} onClick={() => store.getState().setTodosShown(session, !shown)} className="group flex h-5 w-16 shrink-0 cursor-pointer touch-manipulation select-none items-center justify-center rounded-md outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50">
          <span className="h-1 w-9 rounded-full bg-fg-tertiary/40 transition-colors group-hover:bg-fg-secondary group-active:bg-fg-secondary" />
        </button>
        <div className="pointer-events-none relative z-10 -ml-3 flex min-w-0 items-end gap-3 [&>*]:pointer-events-auto">
          <div className="flex min-w-0 flex-wrap-reverse items-end justify-start gap-x-[7px] py-0.5">
            <BackgroundDashes session={session} />
          </div>
          {trailing && <div className="flex shrink-0 items-center">{trailing}</div>}
        </div>
      </div>
    </>
  )
}

/** Mounted only while the visible Jobs card is expanded. */
function JobsRows({ session }: { session: string }) {
  const api = useApi()
  const canList = useQuerySupport(['list_jobs'])
  const canKill = useQuerySupport(['cancel_job'])
  const [killing, setKilling] = useState<string | null>(null)
  const query = useQuery({ queryKey: queryKeys.jobs(session), queryFn: () => api.list_jobs(session), enabled: canList, refetchInterval: 2000, staleTime: 0 })
  const now = useClock(true)
  const kill = async (id: string) => {
    setKilling(id)
    try {
      await api.cancel_job(session, id)
      await query.refetch()
    } catch (e) {
      toast.error(String(e))
    } finally {
      setKilling(null)
    }
  }
  const rows = query.data?.filter((j) => j.state === 'running') ?? []
  return (
    <>
      {!canList ? <p role="status">Jobs unavailable on this engine.</p> : query.error ? <p role="alert">{query.error.message}</p> : query.isPending ? <p role="status">Loading jobs…</p> : !rows.length ? <p role="status">No running jobs.</p> : (
        <ul>{rows.map((job) => (
          <li key={job.id} className="flex min-h-8 items-center gap-2 py-1">
            <ActivityDot color={backgroundColors[job.pty ? 'terminals' : 'jobs'].bg} />
            <span className="min-w-0 flex-1 truncate font-mono" title={job.command}>{job.command}</span>
            <span className="shrink-0 text-fg-tertiary tabular-nums">{fmtThought(Number(job.secs) * 1000 + Math.max(0, now - query.dataUpdatedAt)) ?? '0s'}</span>
            {job.pty ? <TerminalLink session={session} job={job.id} label={`Open terminal ${job.id}`} className="shrink-0 rounded border px-1.5 py-0.5 hover:bg-layer-2">Open</TerminalLink> : canKill && <button type="button" disabled={killing !== null} onClick={() => void kill(job.id)} className="shrink-0 rounded border px-1.5 py-0.5 hover:bg-layer-2 disabled:opacity-50">{killing === job.id ? 'Killing…' : 'Kill'}</button>}
          </li>
        ))}</ul>
      )}
      <button type="button" className="mt-1 block w-full text-right text-fg-tertiary hover:text-foreground" onClick={() => { usePanes.getState().focus(session); openSurface('jobs') }}>All jobs in ⋯ › Jobs</button>
    </>
  )
}

function ReminderRow({ session, id, reminder, now }: { session: string; id: string; reminder?: PendingReminderView; now: number }) {
  const api = useApi()
  const { sessions: store } = useScope()
  const canCancel = useQuerySupport(['cancel_reminder'])
  const [cancelling, setCancelling] = useState(false)
  const cancel = async () => {
    setCancelling(true)
    try {
      await api.cancel_reminder(session, id)
      store.getState().applyEvents([{ type: 'ReminderCancelled', session, id }])
    } catch (e) {
      toast.error(String(e))
    } finally {
      setCancelling(false)
    }
  }
  return (
    <li className="flex min-h-8 items-center gap-2 py-1">
      <ActivityDot color={backgroundColors.reminders.bg} />
      <span className="min-w-0 flex-1 break-words">{reminder?.message ?? `${id} · details unavailable`}</span>
      <span className="shrink-0 text-fg-tertiary tabular-nums" title={reminder ? new Date(reminder.fires_at * 1000).toLocaleString() : undefined}>{reminder ? countdown(reminder.fires_at, now) : '—'}</span>
      {canCancel && <button type="button" disabled={cancelling} onClick={() => void cancel()} className="shrink-0 rounded border px-1.5 py-0.5 hover:bg-layer-2 disabled:opacity-50">{cancelling ? 'Cancelling…' : 'Cancel'}</button>}
    </li>
  )
}
