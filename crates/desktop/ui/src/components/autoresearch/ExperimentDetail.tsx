import { UnifiedDiff } from '@/components/attention/UnifiedDiff'
import { Button } from '@/components/ui/button'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { Skeleton } from '@/components/ui/skeleton'
import { cn } from '@/lib/utils'
import type { BenchSample } from '@/rpc/bindings/BenchSample'
import type { ExperimentDetail as Detail } from '@/rpc/bindings/ExperimentDetail'
import type { StudyDetail } from '@/rpc/bindings/StudyDetail'
import { VerdictBadge } from './RunTable'
import { clock, duration, metric, pct, reviewWord, rowDuration, shortSha, usd } from './model'
import { useControl, useExperiment } from './queries'

/**
 * §13.1 experiment detail, as a sheet over the Study view (`?n=` in the URL, so it is linkable and Back closes it):
 * verdict line, idea and hypothesis, what the model wrote, samples with load, cases moved, deviations + justification,
 * review verdict, Restore for a reverted one, the diff (`UnifiedDiff`), log tails.
 */
export function ExperimentSheet({ study, n, onClose }: { study: StudyDetail; n: number | null; onClose: () => void }) {
  const q = useExperiment(study.info.slug, n)
  return (
    <Sheet open={n !== null} onOpenChange={(open) => !open && onClose()}>
      <SheetContent className="flex w-full flex-col gap-0 overflow-hidden p-0 sm:max-w-2xl">
        <SheetHeader className="shrink-0 border-b border-border-subtle px-5 pt-5 pb-3">
          <SheetTitle className="flex flex-wrap items-center gap-2 text-base">
            Experiment {n}
            {q.data && <VerdictBadge row={q.data.row} />}
            {q.data?.row.commit_sha && <code className="font-mono text-xs text-fg-tertiary">{shortSha(q.data.row.commit_sha)}</code>}
          </SheetTitle>
          <SheetDescription className="text-[13px]">{q.data ? q.data.line : q.isError ? q.error.message : '…'}</SheetDescription>
        </SheetHeader>
        <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4">
          {q.isPending ? (
            <div className="space-y-3">
              <Skeleton className="h-5 w-2/3" />
              <Skeleton className="h-24 w-full" />
              <Skeleton className="h-40 w-full" />
            </div>
          ) : q.isError ? null : (
            <Body d={q.data} study={study} />
          )}
        </div>
      </SheetContent>
    </Sheet>
  )
}

function Body({ d, study }: { d: Detail; study: StudyDetail }) {
  const unit = study.spec.metric.unit
  const control = useControl(study.info.slug)
  const r = d.row
  const reverted = r.state === 'rejected_by_review'
  const live = study.info.state.state === 'running' || study.info.state.state === 'paused'
  const review = r.review ? reviewWord(r.review) : null
  return (
    <div className="flex flex-col gap-5 text-[13px]">
      <section>
        <p>
          <span className="text-[11px] text-fg-tertiary">[{r.category}]</span> <span className="font-medium">{r.idea}</span>
        </p>
        {d.hypothesis && <p className="mt-1 text-fg-secondary">{d.hypothesis}</p>}
        <dl className="mt-2 grid grid-cols-2 gap-x-4 gap-y-1 text-xs text-fg-secondary sm:grid-cols-4">
          <Stat label="Δ vs incumbent" value={pct(r.delta_pct)} className={r.state === 'kept' ? 'text-success' : undefined} />
          <Stat label="samples" value={String(r.n_samples)} />
          <Stat label="took" value={r.finished_at ? duration(rowDuration(r)) : 'running'} />
          <Stat label="cost" value={usd(r.cost_usd)} />
        </dl>
      </section>

      {(review || reverted) && (
        <section className={cn('rounded-md border p-3', review?.tone === 'suspicious' || reverted ? 'border-warning-border/50 bg-warning-bg' : 'border-border-subtle bg-layer-1')}>
          <h4 className="text-xs font-semibold tracking-wide text-fg-secondary uppercase">Review</h4>
          <p className="mt-1">
            {review && r.review ? (
              <>
                <b>{review.word}</b> — {r.review.reason}
              </>
            ) : (
              'reverted by review'
            )}
          </p>
          {reverted && live && (
            <div className="mt-2 flex gap-1.5">
              <Button size="xs" variant="outline" disabled={control.isPending} onClick={() => control.mutate({ op: 'restore', n: r.n })} title="Re-applies the patch as the next experiment and measures it again">
                Restore
              </Button>
            </div>
          )}
        </section>
      )}

      {r.deviations.length > 0 && (
        <section className="rounded-md border border-warning-border/50 bg-warning-bg p-3">
          <h4 className="text-xs font-semibold tracking-wide text-fg-secondary uppercase">Outside scope</h4>
          <ul className="mt-1 font-mono text-xs">
            {r.deviations.map((p) => (
              <li key={p}>{p}</li>
            ))}
          </ul>
          <p className="mt-1">{d.justification ? <>Justification: “{d.justification}”</> : 'No justification given.'}</p>
        </section>
      )}

      {(d.description || d.learned || d.next) && (
        <section className="flex flex-col gap-1.5">
          {d.description && <Note label="Did">{d.description}</Note>}
          {d.learned && <Note label="Learned">{d.learned}</Note>}
          {d.next && <Note label="Next">{d.next}</Note>}
          {d.decision && d.decision !== 'auto' && <Note label="Asked">{d.decision === 'discard' ? 'discard — the model distrusted its own improvement' : 'keep simpler — equal performance, fewer lines (verified by the harness)'}</Note>}
        </section>
      )}

      {d.samples.length > 0 && (
        <section>
          <h4 className="mb-1 text-xs font-semibold tracking-wide text-fg-secondary uppercase">Samples</h4>
          <table className="w-full text-xs">
            <thead className="text-left text-fg-tertiary">
              <tr>
                <th className="py-1 pr-3 font-semibold">#</th>
                <th className="py-1 pr-3 font-semibold">tree</th>
                <th className="py-1 pr-3 text-right font-semibold">{study.spec.metric.name}</th>
                <th className="py-1 pr-3 text-right font-semibold">wall</th>
                <th className="py-1 pr-3 text-right font-semibold" title="1-minute load average ÷ cores at the start of the sample">
                  load
                </th>
                <th className="py-1 font-semibold" />
              </tr>
            </thead>
            <tbody>
              {d.samples.map((s) => (
                <SampleRow key={`${s.role}-${s.k}-${s.at}`} s={s} unit={unit} />
              ))}
            </tbody>
          </table>
        </section>
      )}

      {d.cases_moved.length > 0 && (
        <section>
          <h4 className="mb-1 text-xs font-semibold tracking-wide text-fg-secondary uppercase">Cases that moved most</h4>
          <ul className="flex flex-col gap-0.5 text-xs">
            {d.cases_moved.map((c) => (
              <li key={c.id} className="flex items-baseline gap-2">
                <code className="min-w-0 flex-1 truncate font-mono">{c.id}</code>
                <span className="tabular-nums text-fg-secondary">
                  {metric(c.before, '')} → {metric(c.after, '')}
                </span>
                <span className={cn('w-16 text-right tabular-nums', c.delta_pct === null ? 'text-fg-tertiary' : c.delta_pct < 0 ? 'text-success' : c.delta_pct > 0 ? 'text-danger' : 'text-fg-tertiary')}>{pct(c.delta_pct)}</span>
              </li>
            ))}
          </ul>
        </section>
      )}

      {d.diff && (
        <section>
          <h4 className="mb-1 text-xs font-semibold tracking-wide text-fg-secondary uppercase">{r.state === 'kept' ? 'Commit' : 'Candidate patch'}</h4>
          <UnifiedDiff text={d.diff} />
        </section>
      )}

      {d.tails.map((tail, i) => (
        <section key={i}>
          <h4 className="mb-1 text-xs font-semibold tracking-wide text-fg-secondary uppercase">{d.tails.length > 1 ? `log tail ${i + 1} of ${d.tails.length}` : 'log tail'}</h4>
          <pre className="max-h-48 overflow-auto rounded-md bg-layer-2 p-2 font-mono text-xs leading-5 whitespace-pre-wrap">{tail}</pre>
        </section>
      ))}
    </div>
  )
}

function SampleRow({ s, unit }: { s: BenchSample; unit: string }) {
  return (
    <tr className="border-t border-border-subtle">
      <td className="py-1 pr-3 tabular-nums text-fg-secondary">{s.k}</td>
      <td className="py-1 pr-3 text-fg-secondary" title={s.tree_sha}>
        {s.role ?? shortSha(s.tree_sha)}
      </td>
      <td className="py-1 pr-3 text-right tabular-nums">{s.value === null ? (s.exit !== null ? `exit ${s.exit}` : '—') : metric(s.value, unit)}</td>
      <td className="py-1 pr-3 text-right tabular-nums text-fg-secondary">{clock(s.wall_ms)}</td>
      <td className="py-1 pr-3 text-right tabular-nums text-fg-secondary">{s.load === null ? '—' : s.load.toFixed(2)}</td>
      <td className="py-1 text-fg-tertiary">{s.lost ? 'lost' : s.noisy ? 'noisy — excluded from σ' : ''}</td>
    </tr>
  )
}

function Stat({ label, value, className }: { label: string; value: string; className?: string }) {
  return (
    <div>
      <dt className="text-[11px] text-fg-tertiary">{label}</dt>
      <dd className={cn('tabular-nums text-foreground', className)}>{value}</dd>
    </div>
  )
}

function Note({ label, children }: { label: string; children: string }) {
  return (
    <p>
      <span className="text-[11px] font-semibold tracking-wide text-fg-tertiary uppercase">{label}</span> {children}
    </p>
  )
}
