import { getRouteApi, useNavigate } from '@tanstack/react-router'
import { Flag, OctagonX, Pause, Play, SkipForward, StepForward } from 'lucide-react'
import { type ReactNode, useEffect, useState } from 'react'
import { SectionRow } from '@/components/nav/SectionRow'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { cn } from '@/lib/utils'
import { ExperimentSheet } from './ExperimentDetail'
import { DiscardStudy, Finished } from './Finished'
import { RunTable } from './RunTable'
import { Panel, SidePanel } from './SidePanel'
import { StudyChart } from './StudyChart'
import { StatePill } from './StudyList'
import { type Bar, PAUSE_REASON_LABEL, budgetBars, clock, metric, pct, phaseChips, shortSha } from './model'
import { useAutoresearchEvents, useControl, useExperiments, useStudy } from './queries'
import type { ControlOp, CurrentExperiment, StudyDetail, StudyInfo } from './types'

const route = getRouteApi('/autoresearch/$slug')

/**
 * §13.1 the Study view: header (goal, metric, state pill, controls), stats, three budget bars, the chart, the current
 * experiment with its live tail, the run table, the side panel; the finished block on top once the study ends; the
 * experiment sheet from `?n=`.
 */
export function StudyView() {
  const { slug } = route.useParams()
  const { n } = route.useSearch()
  const navigate = useNavigate()
  const study = useStudy(slug)
  const runs = useExperiments(slug)
  useAutoresearchEvents()
  const pick = (next: number | null) => void navigate({ to: '/autoresearch/$slug', params: { slug }, search: next === null ? {} : { n: next }, replace: next !== null && n !== undefined })
  const back = { to: '/autoresearch' as const, label: 'Studies', className: 'md:hidden' }

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
  const unit = info.metric.unit
  const better = info.delta_pct !== null && (info.metric.direction === 'lower' ? info.delta_pct < 0 : info.delta_pct > 0)
  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <SectionRow
        back={back}
        title={
          <span className="flex items-center gap-2">
            <span className="truncate">{slug}</span>
            <StatePill info={info} currentN={d.current?.n} />
            <span className="hidden rounded-full border border-border px-1.5 text-[10px] font-normal text-fg-secondary sm:inline">study</span>
          </span>
        }
      >
        <Controls info={info} current={d.current} />
      </SectionRow>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto flex w-full max-w-6xl flex-col gap-3 px-4 py-4">
          <p className="text-[13px] text-fg-secondary">
            {info.goal} · <b className="text-foreground">{info.metric.name}</b>, {info.metric.direction} is better · {d.spec.repo.split('/').filter(Boolean).pop()} @ <code className="font-mono text-xs">{shortSha(d.base_sha)}</code>
            {d.spec.checks ? null : <span className="ml-1 text-warning">· no correctness check</span>}
          </p>

          {info.state === 'paused' && info.reason && info.reason !== 'user' && <PausedBanner reason={info.reason} slug={slug} />}
          {info.state === 'failed' && (
            <div className="rounded-md border border-danger-border/50 bg-danger-bg px-3 py-2 text-[13px]">
              <b>Did not start.</b> {info.reason}
            </div>
          )}

          {info.state === 'finished' && <Finished detail={d} />}

          <div className="grid grid-cols-2 gap-2.5 md:grid-cols-4">
            <Stat label="Incumbent" sub={info.base_value !== null ? `base ${metric(info.base_value, unit)}` : 'measuring the baseline'}>
              {metric(info.incumbent_value, unit)} {info.delta_pct !== null && <span className={cn('text-sm font-semibold', better ? 'text-success' : 'text-fg-secondary')}>{pct(info.delta_pct)}</span>}
            </Stat>
            <Stat label="Noise σ" sub={d.sigma !== null ? `${pct(d.sigma_pct, 1).replace(/^[+−]/, '')} · a keep needs ≈ ${pct(d.keep_needs_pct, 1).replace(/^[+−]/, '')}` : `${d.spec.bench.repeats} baseline samples`}>
              {metric(d.sigma, unit)}
            </Stat>
            <Stat label="Kept" sub={[d.counts.reverted > 0 && `${d.counts.reverted} reverted by review`, d.counts.crashed > 0 && `${d.counts.crashed} crashed`].filter(Boolean).join(' · ') || 'nothing reverted'}>
              {d.counts.kept} / {info.experiments}
            </Stat>
            <Stat label="Holdout" sub={d.holdout ? (d.holdout.overfit ? 'overfit: holdout disagrees' : d.holdout.at_keep !== null ? `checked every ${d.spec.holdout?.every ?? '?'} keeps · no overfit` : 'not yet measured') : d.spec.holdout ? 'not yet measured' : 'none configured'}>
              {d.holdout?.latest !== null && d.holdout?.latest !== undefined ? (
                <>
                  {metric(d.holdout.latest, unit)} <span className={cn('text-sm font-semibold', d.holdout.overfit ? 'text-danger' : 'text-success')}>{pct(d.holdout.delta_pct)}</span>
                </>
              ) : (
                '—'
              )}
            </Stat>
          </div>

          <Bars bars={budgetBars(d.budget)} />

          <Panel title="Every experiment" hint="the incumbent as a staircase">
            <StudyChart detail={d} onPick={pick} />
          </Panel>

          <div className="grid min-w-0 grid-cols-1 gap-3 lg:grid-cols-[minmax(0,1fr)_300px]">
            <div className="flex min-w-0 flex-col gap-3">
              {d.current && <Current cur={d.current} onOpen={() => pick(d.current?.n ?? null)} />}
              {info.state === 'preparing' && !d.current && <Preparing detail={d} />}
              <RunTable rows={runs.data} metricName={info.metric.name} pending={runs.isPending} active={n ?? null} onPick={pick} />
            </div>
            <SidePanel detail={d} onPick={pick} />
          </div>
        </div>
      </div>
      <ExperimentSheet study={d} n={n ?? null} onClose={() => pick(null)} />
    </div>
  )
}

/** The §8.6 buttons per state. Labels say their consequence; no second confirmation but Discard's. */
export function Controls({ info, current }: { info: StudyInfo; current: CurrentExperiment | null }) {
  const control = useControl(info.slug)
  const run = (op: ControlOp) => () => control.mutate(op)
  const busy = control.isPending
  const inFlight = current !== null
  if (info.state === 'running') {
    return (
      <div className="ml-auto flex shrink-0 flex-wrap items-center justify-end gap-1.5">
        <Button variant="outline" size="xs" disabled={busy} onClick={run('Pause')} title="Finishes the experiment in flight; nothing new fires">
          <Pause /> Pause
        </Button>
        <Button variant="outline" size="xs" disabled={busy} onClick={run('StopAfter')} title="Finalize once the experiment in flight has closed">
          <SkipForward /> Stop after this
        </Button>
        <Button variant="outline" size="xs" className="text-danger hover:text-danger" disabled={busy} onClick={run('StopNow')} title="Cancels the session, resets the worktree, finalizes">
          <OctagonX /> <span className="hidden xl:inline">Stop now — discard the experiment in flight</span>
          <span className="xl:hidden">Stop now</span>
        </Button>
        <Button variant="ghost" size="xs" disabled={busy || inFlight} onClick={run('Finalize')} title="Skip straight to confirm, changesets and the report">
          <Flag /> Finalize now
        </Button>
      </div>
    )
  }
  if (info.state === 'paused') {
    return (
      <div className="ml-auto flex shrink-0 flex-wrap items-center justify-end gap-1.5">
        <Button size="xs" disabled={busy} onClick={run('Resume')}>
          <Play /> Resume
        </Button>
        <Button variant="outline" size="xs" disabled={busy} onClick={run('RunOnce')} title="One experiment, then paused again">
          <StepForward /> Run one
        </Button>
        {inFlight && (
          <Button variant="outline" size="xs" className="text-danger hover:text-danger" disabled={busy} onClick={run('StopNow')}>
            <OctagonX /> Stop now
          </Button>
        )}
        <Button variant="ghost" size="xs" disabled={busy} onClick={run('Finalize')}>
          <Flag /> Finalize now
        </Button>
      </div>
    )
  }
  if (info.state === 'preparing') {
    return (
      <div className="ml-auto flex shrink-0 items-center gap-1.5">
        <Button variant="outline" size="xs" className="text-danger hover:text-danger" disabled={busy} onClick={run('StopNow')}>
          <OctagonX /> Stop now
        </Button>
      </div>
    )
  }
  if (info.state === 'finalizing') return <span className="ml-auto text-xs text-fg-tertiary">finalizing: confirm runs, changesets, report…</span>
  if (info.state === 'failed' || info.state === 'discarded') return <DiscardStudy slug={info.slug} pending={busy} onConfirm={run('Discard')} />
  return null
}

function PausedBanner({ reason, slug }: { reason: string; slug: string }) {
  const control = useControl(slug)
  const text: Record<string, string> = {
    crash_streak: 'The benchmark failed four times in a row; look at the last crash before resuming.',
    custody_refusals: 'Two experiments in a row changed evaluator files. Resume to let it try again, or open a new segment if you changed the benchmark yourself.',
    review_flags: 'Three keeps were reverted by review: something in the setup invites gaming. Check the reverted experiments before resuming.',
    budget_confirm: 'The study is about to pass its money budget. Resume confirms spending beyond it.',
  }
  return (
    <div className="flex flex-wrap items-center gap-2 rounded-md border border-warning-border/50 bg-warning-bg px-3 py-2 text-[13px]">
      <span>
        <b>Paused: {PAUSE_REASON_LABEL[reason] ?? reason}.</b> {text[reason] ?? ''}
      </span>
      {reason === 'custody_refusals' && (
        <Button variant="outline" size="xs" className="ml-auto" disabled={control.isPending} onClick={() => control.mutate({ Segment: { reason: 'evaluator changed by the user' } })} title="New custody copies and a fresh baseline; σ restarts">
          Open a new segment
        </Button>
      )}
    </div>
  )
}

function Stat({ label, sub, children }: { label: string; sub?: string; children: ReactNode }) {
  return (
    <div className="rounded-[10px] border border-border-subtle bg-layer-1 px-3 py-2">
      <div className="text-[11px] tracking-wide text-fg-tertiary uppercase">{label}</div>
      <div className="mt-0.5 text-lg font-semibold tabular-nums">{children}</div>
      {sub && <div className="truncate text-xs text-fg-secondary" title={sub}>{sub}</div>}
    </div>
  )
}

/** Three thin bars: money, time, experiments; the fill is the accent, full bars turn carnelian. */
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

/** The experiment in flight: phase chips, the elapsed clock (ticks locally between events), the idea, the live sample-log tail. */
function Current({ cur, onOpen }: { cur: CurrentExperiment; onOpen: () => void }) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(t)
  }, [])
  const elapsed = Math.max(cur.elapsed_ms, now - Date.parse(cur.started_at))
  const chips = phaseChips(cur)
  const on = chips.find((c) => c.status === 'on')
  return (
    <Panel
      title={
        <span className="tabular-nums">
          Experiment {cur.n} · {on?.label} · {clock(elapsed)}
        </span>
      }
      actions={
        <Button variant="ghost" size="xs" onClick={onOpen}>
          Open
        </Button>
      }
    >
      <div className="flex flex-wrap gap-1.5" aria-label="Phases">
        {chips.map((c) => (
          <span key={c.label} className={cn('rounded-md px-2 py-0.5 text-[11.5px]', c.status === 'on' ? 'bg-info-bg font-semibold text-info' : c.status === 'done' ? 'bg-layer-2 text-success' : 'bg-layer-2 text-fg-tertiary')}>
            {c.label}
          </span>
        ))}
      </div>
      {cur.idea && (
        <p className="mt-2 text-[13px]">
          {cur.category && <span className="text-[11px] text-fg-tertiary">[{cur.category}]</span>} {cur.idea}
          {cur.note && <span className="ml-1 text-xs text-fg-secondary">— {cur.note}</span>}
        </p>
      )}
      {!cur.idea && <p className="mt-2 text-[13px] text-fg-tertiary">Reading the brief and editing; the idea is declared at the first measurement.</p>}
      {cur.tail && (
        <pre className="mt-2 max-h-28 overflow-auto rounded-md bg-layer-2 p-2 font-mono text-xs leading-5 whitespace-pre-wrap" aria-label="Sample log tail">
          {cur.tail}
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
          Worktree on <code className="font-mono text-xs">autoresearch/{detail.info.slug}</code> from <code className="font-mono text-xs">{shortSha(detail.base_sha)}</code>
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
