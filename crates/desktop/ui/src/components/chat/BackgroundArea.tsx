import { useQuery } from '@tanstack/react-query'
import { type ReactNode, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { motionAllowed } from '@/lib/animations'
import { cn } from '@/lib/utils'
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
import { KernelStrip, useSessionKernels } from '@/components/cells/KernelStrip'
import { BackgroundCard } from './BackgroundCard'
import { ActivityDot, BackgroundDashes, SubagentDashes, SubagentLink, TerminalLink } from './SubagentStrip'
import { openSurface, surfaceOf } from './slash'
import { TodoCard } from './TodoCard'
import { TrayTiles, trayHasItems, useTray } from './TrayItems'

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
 * One grabber, fixed against the composer; every populated category opens above it: the to-dos, the tiles (terminals,
 * kernels, browser, artifacts, subagents, jobs, desktop, CodeMap — the panels moved out of the header), then the cards.
 * The activity dashes grow out of the bar's ends on its row (subagents left; jobs · terminals · kernels · browsers ·
 * artifacts · desktop · reminders right), the `trailing` Advisor pill last.
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
  const kernelsExpanded = useSessionField(session, 'kernelsExpanded') ?? false
  const kernels = useSessionKernels(session)
  const now = useClock(shown)
  const tray = useTray(session)
  const { mounted, phase, shellRef, stageRef, pillRef } = useHorizonWarp(session, shown)
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
  const busyKernels = kernels.filter((k) => k.phase !== 'ready').length
  // `agents` (liveRows) is the uncapped stripRows, so every dash the row could show keeps the bar visible; so does any
  // panel (`trayHasItems`), the bar being its only way in.
  if (!hasTodos && !agents.length && !jobCount && !terminalCount && !kernels.length && !reminderCount && !trayHasItems(tray)) {
    return trailing ? <div className="mx-auto flex min-h-7 w-full shrink-0 select-none items-center justify-center px-4 text-xs text-fg-tertiary md:max-w-[768px]">{trailing}</div> : null
  }
  const waiting = agents.filter((a) => a.state.state === 'approval').length
  const pending = [...(reminders ?? [])].map((id) => details?.get(id)).filter((r): r is PendingReminderView => !!r)
  const next = pending.length ? Math.min(...pending.map((r) => r.fires_at)) : null
  const blockId = `background-${session}`
  const label = shown ? 'Hide background activity' : 'Show background activity'
  const exiting = phase === 'exiting'
  return (
    <>
      {mounted && (
        <div
          id={blockId}
          ref={shellRef}
          aria-hidden={exiting || undefined}
          className={cn(
            'mx-auto w-full max-w-[768px] shrink-0 px-4',
            phase === 'open' ? 'max-h-[calc(var(--visible-band,100dvh)/2)] overflow-y-auto' : 'overflow-hidden',
            exiting && 'pointer-events-none',
          )}
        >
          <div
            ref={stageRef}
            className={cn(
              'horizon-stage flex flex-col gap-1.5 pb-1',
              phase === 'entering' && 'horizon-in',
              exiting && 'horizon-out',
            )}
          >
            {hasTodos && <TodoCard session={session} />}
            <TrayTiles tray={tray} />
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
                {jobsExpanded && <JobsRows session={session} active={shown} />}
              </BackgroundCard>
            )}
            {kernels.length > 0 && (
              <BackgroundCard title="Kernels" color={backgroundColors.kernels.bg} summary={`${kernels.length} live${busyKernels ? ` · ${busyKernels} busy` : ''}`} expanded={kernelsExpanded} onExpanded={(open) => store.getState().setBackgroundExpanded(session, 'kernels', open)}>
                {kernelsExpanded && <KernelStrip session={session} inBar />}
              </BackgroundCard>
            )}
            {reminderCount > 0 && (
              <BackgroundCard title="Reminders" color={backgroundColors.reminders.bg} summary={`${reminderCount} pending${next === null ? '' : ` · ${countdown(next, now) === 'due now' ? 'due now' : `in ${countdown(next, now)}`}`}`} expanded={remindersExpanded} onExpanded={(open) => store.getState().setBackgroundExpanded(session, 'reminders', open)}>
                {remindersExpanded && <ul>{[...(reminders ?? [])].map((id) => <ReminderRow key={id} session={session} id={id} reminder={details?.get(id)} now={now} />)}</ul>}
              </BackgroundCard>
            )}
          </div>
        </div>
      )}
      {/* 1fr auto 1fr keeps the bar centred whatever each side holds. A side that runs out of width wraps upward
          (`flex-wrap-reverse`): the first dashes stay on the bottom row, level with the bar (`items-end`), the rest
          stack above in the same order; nothing overflows the pane sideways. Their -m-3 lets the dashes reach into the
          button's padding so they sit a dash-gap from the bar's ends. */}
      <div data-dash-row="" data-open={shown ? 'true' : undefined} className="mx-auto grid min-h-6 w-full max-w-[768px] shrink-0 select-none grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] items-end px-4 text-xs text-fg-tertiary">
        <div className="tray-dash-side tray-dash-left pointer-events-none relative z-10 -mr-3 flex min-w-0 items-end justify-end py-1 [&>*]:pointer-events-auto">
          <SubagentDashes session={session} />
        </div>
        <button type="button" aria-expanded={shown} aria-controls={blockId} aria-label={label} title={label} onClick={() => store.getState().setTodosShown(session, !shown)} className="group flex h-6 w-20 shrink-0 cursor-pointer touch-manipulation select-none items-center justify-center rounded-md outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50">
          <span ref={pillRef} className="tray-grabber-pill h-[5px] w-12 rounded-full bg-fg-tertiary/40 group-hover:bg-fg-secondary group-active:bg-fg-secondary" />
        </button>
        <div className="tray-dash-side tray-dash-right pointer-events-none relative z-10 -ml-3 flex min-w-0 items-end gap-3 [&>*]:pointer-events-auto">
          <div className="flex min-w-0 flex-wrap-reverse items-end justify-start gap-x-[7px] py-1">
            <BackgroundDashes session={session} tray={tray} />
          </div>
          {trailing && <div className="flex shrink-0 items-center">{trailing}</div>}
        </div>
      </div>
    </>
  )
}

type HorizonPhase = 'closed' | 'entering' | 'open' | 'exiting'
const HORIZON_OPEN_MS = 440
const HORIZON_CLOSE_MS = 320



/**
 * 3 · Folded Horizon (1x) lifecycle for the session details drawer above the grabber:
 * - Opening measures the natural height and glides the shell `0 → H` while the inner stage hinges up (`horizon-in`).
 * - Closing locks the current height, glides `H → 0` while folding flat (`horizon-out`), pulses the grabber pill on
 *   absorption, then unmounts. Under reduced motion or zero-height headless layouts (jsdom), open/close settle instantly.
 */
function useHorizonWarp(session: string, shown: boolean) {
  const [phase, setPhase] = useState<HorizonPhase>(shown ? 'open' : 'closed')
  const shellRef = useRef<HTMLDivElement>(null)
  const stageRef = useRef<HTMLDivElement>(null)
  const pillRef = useRef<HTMLSpanElement>(null)
  const prev = useRef({ session, shown })
  const timers = useRef<number[]>([])

  const clearTimers = () => {
    for (const id of timers.current) clearTimeout(id)
    timers.current = []
  }

  const pulsePill = (cls: 'tray-pill-launch' | 'tray-pill-absorb') => {
    const pill = pillRef.current
    if (!pill || !motionAllowed()) return
    pill.classList.remove('tray-pill-launch', 'tray-pill-absorb')
    void pill.offsetWidth
    pill.classList.add(cls)
  }

  useLayoutEffect(() => {
    const sameSession = prev.current.session === session
    const wasShown = prev.current.shown
    prev.current = { session, shown }

    if (!sameSession) {
      clearTimers()
      if (shellRef.current) {
        shellRef.current.style.transition = ''
        shellRef.current.style.height = ''
      }
      setPhase(shown ? 'open' : 'closed')
      return
    }
    if (wasShown === shown) return
    clearTimers()

    if (shown) {
      if (!motionAllowed()) {
        setPhase('open')
        return
      }
      pulsePill('tray-pill-launch')
      setPhase('entering')
      return
    }

    const shell = shellRef.current
    const startH = shell?.getBoundingClientRect().height ?? 0
    if (!motionAllowed() || !shell || startH <= 0) {
      if (shell) {
        shell.style.transition = ''
        shell.style.height = ''
      }
      setPhase('closed')
      return
    }

    shell.style.transition = 'none'
    shell.style.height = `${startH}px`
    void shell.offsetHeight
    shell.style.transition = `height ${HORIZON_CLOSE_MS}ms cubic-bezier(0.55, 0.05, 0.85, 0.3)`
    shell.style.height = '0px'
    setPhase('exiting')

    timers.current.push(
      window.setTimeout(() => pulsePill('tray-pill-absorb'), HORIZON_CLOSE_MS - 90),
      window.setTimeout(() => {
        if (shellRef.current) {
          shellRef.current.style.transition = ''
          shellRef.current.style.height = ''
        }
        setPhase('closed')
      }, HORIZON_CLOSE_MS + 25),
    )
  }, [session, shown])

  useLayoutEffect(() => {
    if (phase !== 'entering') return
    const shell = shellRef.current
    const stage = stageRef.current
    if (!shell || !stage) return

    shell.style.transition = 'none'
    shell.style.height = 'auto'
    const targetH = stage.getBoundingClientRect().height
    if (targetH <= 0) {
      shell.style.transition = ''
      shell.style.height = ''
      setPhase('open')
      return
    }

    const maxH = typeof window !== 'undefined' ? Math.max(120, Math.floor(window.innerHeight / 2)) : targetH
    const clampedH = Math.min(targetH, maxH)
    shell.style.height = '0px'
    void shell.offsetHeight
    shell.style.transition = `height ${HORIZON_OPEN_MS}ms cubic-bezier(0.22, 1, 0.36, 1)`
    shell.style.height = `${clampedH}px`

    const done = window.setTimeout(() => {
      if (shellRef.current) {
        shellRef.current.style.transition = ''
        shellRef.current.style.height = ''
      }
      setPhase('open')
    }, HORIZON_OPEN_MS + 25)
    timers.current.push(done)
  }, [phase])

  useEffect(() => () => clearTimers(), [])

  return { mounted: shown || phase !== 'closed', phase, shellRef, stageRef, pillRef }
}

/** Mounted only while the visible Jobs card is expanded. */
function JobsRows({ session, active }: { session: string; active: boolean }) {
  const api = useApi()
  const canList = useQuerySupport(['list_jobs'])
  const canKill = useQuerySupport(['cancel_job'])
  const [killing, setKilling] = useState<string | null>(null)
  const query = useQuery({ queryKey: queryKeys.jobs(session), queryFn: () => api.list_jobs(session), enabled: canList && active, refetchInterval: active ? 2000 : false, staleTime: 0 })
  const now = useClock(active)
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
