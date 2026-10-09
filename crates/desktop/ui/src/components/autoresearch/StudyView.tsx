import { Link, getRouteApi, useNavigate } from '@tanstack/react-router'
import { Flag, OctagonX, Pause, Play, Scissors, SkipForward, StepForward } from 'lucide-react'
import { type ReactNode, useEffect, useMemo, useState } from 'react'
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle, AlertDialogTrigger } from '@/components/ui/alert-dialog'
import { SectionRow } from '@/components/nav/SectionRow'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Skeleton } from '@/components/ui/skeleton'
import { cn } from '@/lib/utils'
import type { ControlOp } from '@/rpc/bindings/ControlOp'
import type { ExperimentRow } from '@/rpc/bindings/ExperimentRow'
import type { PauseReason } from '@/rpc/bindings/PauseReason'
import type { StudyDetail } from '@/rpc/bindings/StudyDetail'
import type { StudyInfo } from '@/rpc/bindings/StudyInfo'
import { ExperimentSheet } from './ExperimentDetail'
import { DiscardStudy, Finished } from './Finished'
import { RunTable } from './RunTable'
import { Panel, SidePanel } from './SidePanel'
import { StudyChart } from './StudyChart'
import { Delta, StatePill } from './StudyList'
import { type Bar, type Progress, budgetBars, clock, holdoutSummary, keepNeedsPct, metric, pauseReasonLabel, pct, phaseChips, shortSha, tally } from './model'
import { useControl, useExperiments, useProgress, useStudy } from './queries'

const route = getRouteApi('/autoresearch/$slug')

/**
 * §13.1 the Study view: header (goal, metric, state pill, controls), stats, three budget bars, the chart, the current
 * experiment, the run table, the side panel; the finished block on top once the study ends; the experiment sheet
 * from `?n=`.
 */
export function StudyView() {
  const { slug } = route.useParams()
  const { n } = route.useSearch()
  const navigate = useNavigate()
  const study = useStudy(slug)
  const runs = useExperiments(slug)
  const progress = useProgress(slug)
  const pick = (next: number | null) => void navigate({ to: '/autoresearch/$slug', params: { slug }, search: next === null ? {} : { n: next }, replace: next !== null && n !== undefined })
  const back = { to: '/autoresearch' as const, label: 'Studies', className: 'md:hidden' }
  const counts = useMemo(() => tally(runs.data ?? []), [runs.data])

  if (study.isPending) {
    return (
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        <SectionRow back={back} title={slug} />
        <div className="mx-auto w-full max-w-6xl space-y-4 px-4 py-4">
          <Skeleton className="h-6 w-2/3" />
          <Skeleton className="h-20 w-full" />
          <Skeleton className="h-64 w-full" />
        </div>
      </div>
    )
  }
  if (study.isError) {
    return (
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        <SectionRow back={back} title={slug} />
        <p className="m-auto text-sm text-danger">{study.error.message}</p>
      </div>
    )
  }
  const d = study.data
  const info = d.info
  const st = info.state
  const unit = info.metric.unit
  const direction = info.metric.direction
  const holdout = holdoutSummary(d.series)
  const needs = keepNeedsPct(info.sigma, info.incumbent.mean, info.metric.min_effect)
  const current = info.current !== null ? (runs.data?.find((r) => r.n === info.current) ?? null) : null
  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <SectionRow
        back={back}
        title={
          <span className="flex min-w-0 items-center gap-2">
            <span className="truncate">{slug}</span>
            <StatePill state={st} currentN={info.current} stop={info.stop_reason} />
          </span>
        }
      >
        <div className="ml-auto hidden 2xl:flex">
          <Controls info={info} />
        </div>
      </SectionRow>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto flex w-full max-w-6xl flex-col gap-3 px-4 py-4">
          {/* Below 2xl the controls would crowd the slug out of the 44 px row: they get their own line here. */}
          <div className="flex 2xl:hidden">
            <Controls info={info} />
          </div>
          <p className="text-[13px] text-fg-secondary">
            {info.goal} · <b className="text-foreground">{info.metric.name}</b>, {info.metric.direction} is better · {d.spec.repo.split('/').filter(Boolean).pop()}
            {info.base_sha && (
              <>
                {' '}
                @ <code className="font-mono text-xs">{shortSha(info.base_sha)}</code>
              </>
            )}
            {d.spec.checks ? null : <span className="ml-1 text-warning">· no correctness check</span>}
            {info.session && (
              <>
                {' · '}
                <Link to="/c/$session" params={{ session: info.session }} search={{}} className="underline underline-offset-2">
                  study session
                </Link>
              </>
            )}
          </p>

          {st.state === 'paused' && st.reason.kind !== 'user' && <PausedBanner reason={st.reason} slug={slug} />}
          {st.state === 'failed' && (
            <div className="rounded-md border border-danger-border/50 bg-danger-bg px-3 py-2 text-[13px]">
              <b>Did not start.</b> {st.reason}
            </div>
          )}

          {st.state === 'finished' && <Finished detail={d} />}

          <div className="grid grid-cols-2 gap-2.5 2xl:grid-cols-4">
            <Stat label="Incumbent" sub={info.baseline_mean !== null ? `base ${metric(info.baseline_mean, unit)}` : 'measuring the baseline'}>
              {metric(info.incumbent.mean, unit)} {info.delta_pct !== null && <Delta direction={direction} delta_pct={info.delta_pct} className="text-sm font-semibold" />}
            </Stat>
            <Stat label="Noise σ" sub={info.sigma !== null && info.incumbent.mean ? `${pct((info.sigma / Math.abs(info.incumbent.mean)) * 100).replace(/^[+−]/, '')}${needs !== null ? ` · a keep needs ≈ ${pct(needs).replace(/^[+−]/, '')}` : ''}` : `${d.spec.bench.repeats} baseline samples`}>
              {metric(info.sigma, unit)}
            </Stat>
            <Stat label="Kept" sub={[counts.reverted > 0 && `${counts.reverted} reverted by review`, counts.crashed > 0 && `${counts.crashed} crashed`, counts.checks_failed > 0 && `${counts.checks_failed} failed checks`].filter(Boolean).join(' · ') || 'nothing reverted'}>
              {info.incumbent.kept} / {info.experiments}
            </Stat>
            {/* §13 Revision 2: never "base → base" — finalize's holdout once finished, else when it is due. */}
            <Stat
              label="Holdout"
              sub={
                holdout
                  ? info.stop_reason?.kind === 'overfit'
                    ? 'overfit: the holdout disagrees'
                    : `after keep ${holdout.n}${d.spec.holdout ? ` · every ${d.spec.holdout.every} keeps` : ''}`
                  : d.final_holdout
                    ? 'base → final, measured at finalize'
                    : d.spec.holdout
                      ? `holdout not measured yet (every ${d.spec.holdout.every} keeps)`
                      : 'none configured'
              }
            >
              {holdout ? (
                <>
                  {metric(holdout.latest, unit)}{' '}
                  {info.stop_reason?.kind === 'overfit' ? <span className="text-sm font-semibold text-danger">{pct(holdout.delta_pct)}</span> : <Delta direction={direction} delta_pct={holdout.delta_pct} className="text-sm font-semibold" />}
                </>
              ) : (
                (d.final_holdout ?? '—')
              )}
            </Stat>
          </div>

          <Bars bars={budgetBars(info.budget_used, info.budget)} />

          <Panel title="Every experiment" hint="the incumbent as a staircase">
            <StudyChart detail={d} rows={runs.data} onPick={pick} />
          </Panel>

          {info.current !== null && <Current n={info.current} row={current} progress={progress ?? { phase: info.phase, sample: 0, of: 0, elapsed_ms: 0 }} tail={d.tail} onOpen={() => pick(info.current)} />}
          {st.state === 'preparing' && <Preparing detail={d} />}
          {/* Below 2xl the side panel sits above the table as two columns (the steer box stays near the top); at 2xl it is the mock's 300 px column. */}
          <div className="grid min-w-0 grid-cols-1 gap-3 2xl:grid-cols-[minmax(0,1fr)_300px]">
            <div className="order-2 min-w-0 2xl:order-1">
              <RunTable rows={runs.data} metricName={info.metric.name} direction={direction} pending={runs.isPending} active={n ?? null} onPick={pick} />
            </div>
            <div className="order-1 min-w-0 2xl:order-2">
              <SidePanel detail={d} rows={runs.data ?? []} onPick={pick} />
            </div>
          </div>
        </div>
      </div>
      <ExperimentSheet study={d} n={n ?? null} onClose={() => pick(null)} />
    </div>
  )
}

/** The §8.6 buttons per state. Labels say their consequence; no second confirmation but Discard's. */
export function Controls({ info }: { info: StudyInfo }) {
  const control = useControl(info.slug)
  const run = (op: ControlOp) => () => control.mutate(op)
  const busy = control.isPending
  const inFlight = info.current !== null
  switch (info.state.state) {
    case 'running':
      return (
        <div className="flex flex-wrap items-center gap-1.5 2xl:ml-auto 2xl:shrink-0 2xl:justify-end">
          <Button variant="outline" size="xs" disabled={busy} onClick={run({ op: 'pause' })} title="Finishes the experiment in flight; nothing new fires">
            <Pause /> Pause
          </Button>
          <Button variant="outline" size="xs" disabled={busy} onClick={run({ op: 'stop_after' })} title="Finalize once the experiment in flight has closed">
            <SkipForward /> Stop after this
          </Button>
          <Button variant="outline" size="xs" className="text-danger hover:text-danger" disabled={busy} onClick={run({ op: 'stop_now' })} title="Cancels the session, resets the worktree, finalizes">
            <OctagonX /> <span className="hidden 2xl:inline">Stop now — discard the experiment in flight</span>
            <span className="2xl:hidden">Stop now — discards in flight</span>
          </Button>
          <Button variant="ghost" size="xs" disabled={busy || inFlight} onClick={run({ op: 'finalize' })} title="Skip straight to confirm, changesets and the report">
            <Flag /> Finalize now
          </Button>
          <NewSegment slug={info.slug} pending={busy} />
        </div>
      )
    case 'paused':
      return (
        <div className="flex flex-wrap items-center gap-1.5 2xl:ml-auto 2xl:shrink-0 2xl:justify-end">
          <Button size="xs" disabled={busy} onClick={run({ op: 'resume' })}>
            <Play /> Resume
          </Button>
          <Button variant="outline" size="xs" disabled={busy} onClick={run({ op: 'run_once' })} title="One experiment, then paused again">
            <StepForward /> Run one
          </Button>
          {inFlight && (
            <Button variant="outline" size="xs" className="text-danger hover:text-danger" disabled={busy} onClick={run({ op: 'stop_now' })}>
              <OctagonX /> Stop now
            </Button>
          )}
          <Button variant="ghost" size="xs" disabled={busy} onClick={run({ op: 'finalize' })}>
            <Flag /> Finalize now
          </Button>
          <NewSegment slug={info.slug} pending={busy} />
        </div>
      )
    case 'preparing':
      // `stop_now` is refused while preparing (the baseline runs to its end or fails); nothing to press yet.
      return <span className="text-xs text-fg-tertiary 2xl:ml-auto">preparing: worktree, custody, baseline samples…</span>
    case 'finalizing':
      return <span className="text-xs text-fg-tertiary 2xl:ml-auto">finalizing: confirm runs, changesets, report…</span>
    case 'failed':
      return <DiscardStudy slug={info.slug} pending={busy} onConfirm={run({ op: 'discard' })} />
    default:
      return null
  }
}

function PausedBanner({ reason, slug }: { reason: PauseReason; slug: string }) {
  const control = useControl(slug)
  const text: Partial<Record<PauseReason['kind'], string>> = {
    crash_streak: 'The benchmark failed four times in a row; look at the last crash before resuming.',
    custody_refusals: 'Two experiments in a row changed evaluator files. Resume to let it try again, or open a new segment if you changed the benchmark yourself.',
    review_flags: 'Three keeps were reverted by review: something in the setup invites gaming. Check the reverted experiments before resuming.',
    budget_confirm: 'The study is about to pass its money budget. Resume confirms spending beyond it.',
  }
  return (
    <div className="flex flex-wrap items-center gap-2 rounded-md border border-warning-border/50 bg-warning-bg px-3 py-2 text-[13px]">
      <span>
        <b>Paused: {pauseReasonLabel(reason)}.</b> {text[reason.kind] ?? ''}
      </span>
      {reason.kind === 'custody_refusals' && (
        <Button variant="outline" size="xs" className="ml-auto" disabled={control.isPending} onClick={() => control.mutate({ op: 'segment', reason: 'evaluator changed by the user' })} title="New custody copies and a fresh baseline; σ restarts">
          Open a new segment
        </Button>
      )}
    </div>
  )
}

/** §5.3: only the user opens a segment — new custody copies and a fresh baseline on the incumbent, σ restarts; the reason is required. */
export function NewSegment({ slug, pending }: { slug: string; pending: boolean }) {
  const control = useControl(slug)
  const [reason, setReason] = useState('')
  const ready = reason.trim().length > 0
  return (
    <AlertDialog onOpenChange={(open) => !open && setReason('')}>
      <AlertDialogTrigger asChild>
        <Button variant="ghost" size="xs" disabled={pending || control.isPending} title="New custody copies of the evaluator and a fresh baseline; σ restarts">
          <Scissors /> New segment
        </Button>
      </AlertDialogTrigger>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Open a new segment of {slug}?</AlertDialogTitle>
          <AlertDialogDescription>
            Copies the evaluator files again, measures a fresh baseline on the incumbent and restarts σ. Earlier experiments stay visible but leave the statistics. Say why (it goes in the ledger).
          </AlertDialogDescription>
        </AlertDialogHeader>
        <Input value={reason} placeholder="Reason, e.g. fixed the benchmark's warm-up" aria-label="Reason" onChange={(e) => setReason(e.target.value)} />
        <AlertDialogFooter>
          <AlertDialogCancel>Keep</AlertDialogCancel>
          <AlertDialogAction disabled={!ready} onClick={() => control.mutate({ op: 'segment', reason: reason.trim() })}>
            Open segment
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}

function Stat({ label, sub, children }: { label: string; sub?: string; children: ReactNode }) {
  return (
    <div className="rounded-[10px] border border-border-subtle bg-layer-1 px-3 py-2">
      <div className="text-[11px] tracking-wide text-fg-tertiary uppercase">{label}</div>
      <div className="mt-0.5 text-lg font-semibold tabular-nums">{children}</div>
      {sub && (
        <div className="line-clamp-2 text-xs text-fg-secondary" title={sub}>
          {sub}
        </div>
      )}
    </div>
  )
}

/** Three thin bars: money, time, experiments; the fill is the accent, past 85 % ochre, full carnelian. */
export function Bars({ bars }: { bars: Bar[] }) {
  return (
    <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-3">
      {bars.map((b) => (
        <div key={b.label}>
          <div className="flex justify-between text-xs text-fg-secondary">
            <span>{b.label}</span>
            <span className="tabular-nums">{b.text}</span>
          </div>
          <div className="mt-1 h-1.5 overflow-hidden rounded-full bg-layer-2" role="progressbar" aria-label={b.label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(b.fraction * 100)}>
            <div className={cn('h-full rounded-full', b.fraction >= 1 ? 'bg-danger' : b.fraction >= 0.85 ? 'bg-warning' : 'bg-fg-accent')} style={{ width: `${b.fraction * 100}%` }} />
          </div>
        </div>
      ))}
    </div>
  )
}

/**
 * The experiment in flight: phase chips from the last `ExperimentProgress`, the elapsed clock (ticks locally between
 * events, from the row's `started_at` once the row exists), the idea once declared, the running sample's log tail
 * (`StudyDetail.tail`, refetched on every `ExperimentProgress`).
 */
export function Current({ n, row, progress, tail, onOpen }: { n: number; row: ExperimentRow | null; progress: Progress; tail: string | null; onOpen: () => void }) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(t)
  }, [])
  const elapsed = Math.max(progress.elapsed_ms, row ? now - Date.parse(row.started_at) : 0)
  const chips = phaseChips(progress)
  const on = chips.find((c) => c.status === 'on')
  return (
    <Panel
      title={
        <span className="tabular-nums">
          Experiment {n} · {on?.label}
          {elapsed > 0 && ` · ${clock(elapsed)}`}
        </span>
      }
      actions={
        row && (
          <Button variant="ghost" size="xs" onClick={onOpen}>
            Open
          </Button>
        )
      }
    >
      <div className="flex flex-wrap gap-1.5" aria-label="Phases">
        {chips.map((c) => (
          <span key={c.label} className={cn('rounded-md px-2 py-0.5 text-[11.5px]', c.status === 'on' ? 'bg-info-bg font-semibold text-info' : c.status === 'done' ? 'bg-layer-2 text-success' : 'bg-layer-2 text-fg-tertiary')}>
            {c.label}
          </span>
        ))}
      </div>
      {row?.idea ? (
        <p className="mt-2 text-[13px]">
          <span className="text-[11px] text-fg-tertiary">[{row.category}]</span> {row.idea}
        </p>
      ) : (
        <p className="mt-2 text-[13px] text-fg-tertiary">Reading the brief and editing; the idea is declared at the first measurement.</p>
      )}
      {tail && (
        <pre className="mt-2 max-h-40 overflow-auto rounded-md bg-layer-2 p-2 font-mono text-[11px] leading-snug text-fg-secondary" aria-label="Sample log tail">
          {tail}
        </pre>
      )}
    </Panel>
  )
}

function Preparing({ detail }: { detail: StudyDetail }) {
  return (
    <Panel title="Preparing">
      <ol className="list-decimal space-y-0.5 pl-5 text-[13px] text-fg-secondary">
        <li>
          Worktree on <code className="font-mono text-xs">autoresearch/{detail.info.slug}</code> from <code className="font-mono text-xs">{detail.spec.base}</code>
        </li>
        <li>Evaluator copied into custody and hashed</li>
        <li>
          Baseline: {detail.spec.bench.repeats} samples of <code className="font-mono text-xs">{detail.spec.bench.command}</code> → σ
        </li>
      </ol>
      <p className="mt-2 text-xs text-fg-tertiary">The first experiment fires when σ is in; if the spread is above 5 % the study stops here and says so.</p>
    </Panel>
  )
}
