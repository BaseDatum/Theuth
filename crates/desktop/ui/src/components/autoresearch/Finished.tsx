import { Link } from '@tanstack/react-router'
import { Check, Copy, FileText, GitMerge, GitPullRequest, Trash2 } from 'lucide-react'
import { type ReactNode, useState } from 'react'
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle, AlertDialogTrigger } from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import type { StudyChangeset } from '@/rpc/bindings/StudyChangeset'
import type { StudyDetail } from '@/rpc/bindings/StudyDetail'
import { Panel } from './SidePanel'
import { holdoutSummary, metric, pct, reviewWord, stopReasonLabel } from './model'
import { useControl } from './queries'

/**
 * §11.5 the finished state: the headline, the changesets table (Δ alone / Δ together / files / review), the report
 * link, Discard study (one confirmation of the label). Merge into my branch is the `merge` control op (a local
 * `git merge --no-ff`, refused on a dirty tree); Open PR copies the push + `gh pr create` commands — the plugin
 * never pushes (§6).
 */
export function Finished({ detail }: { detail: StudyDetail }) {
  const control = useControl(detail.info.slug)
  const sets = detail.changesets
  const info = detail.info
  const unit = info.metric.unit
  const holdout = holdoutSummary(detail.series)
  // §11.1's headline from the numbers the detail carries (`info.headline` is the routines-row sentence).
  const headline =
    info.baseline_mean !== null && info.incumbent.mean !== null
      ? `${info.metric.name} ${metric(info.baseline_mean, '')} → ${metric(info.incumbent.mean, unit)} (${pct(info.delta_pct)})${holdout ? ` · holdout ${metric(holdout.base, '')} → ${metric(holdout.latest, unit)} (${pct(holdout.delta_pct)})` : ''}`
      : null
  return (
    <Panel
      title="Finished"
      hint={detail.info.stop_reason ? stopReasonLabel(detail.info.stop_reason) : undefined}
      actions={
        <>
          {/* A documents-store id links; a file-path fallback (no store) is named, not linked (R-98). */}
          {detail.info.report_doc && !detail.info.report_doc.includes('/') ? (
            <Button asChild variant="outline" size="xs">
              <Link to="/notes/$id" params={{ id: detail.info.report_doc }} search={{}}>
                <FileText /> Report
              </Link>
            </Button>
          ) : detail.info.report_doc ? (
            <code className="font-mono text-xs text-fg-tertiary" title="The report was written to this file">
              {detail.info.report_doc}
            </code>
          ) : null}
          <DiscardStudy slug={detail.info.slug} pending={control.isPending} onConfirm={() => control.mutate({ op: 'discard' })} />
        </>
      }
      className="border-success-border/40"
    >
      {headline && <p className="mb-3 text-[15px] font-semibold tabular-nums">{headline}</p>}
      {sets.length === 0 ? (
        <p className="text-[13px] text-fg-tertiary">No changeset cleared the gate on its own; nothing to land.</p>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full text-[13px]">
            <thead>
              <tr className="text-left text-[11.5px] font-semibold text-fg-tertiary">
                <th className="px-2 py-1.5">changeset</th>
                <th className="px-2 py-1.5 text-right whitespace-nowrap" title="Measured alone on the base, bench.repeats samples">
                  Δ alone
                </th>
                <th className="px-2 py-1.5 text-right whitespace-nowrap" title="Its share of the final incumbent's improvement">
                  Δ together
                </th>
                <th className="hidden px-2 py-1.5 2xl:table-cell">files</th>
                <th className="px-2 py-1.5">review</th>
                <th className="px-2 py-1.5 text-right" />
              </tr>
            </thead>
            <tbody>
              {sets.map((c) => (
                <Row key={c.k} c={c} slug={detail.info.slug} pending={control.isPending} onMerge={() => control.mutate({ op: 'merge', changeset: c.k })} />
              ))}
            </tbody>
          </table>
        </div>
      )}
      <p className="mt-2 text-xs text-fg-tertiary">
        Each changeset is a local branch (<code className="font-mono">{sets[0]?.branch ?? `autoresearch/${detail.info.slug}+<k>-<name>`}</code>, …); nothing was pushed.
      </p>
    </Panel>
  )
}

function Row({ c, slug, pending, onMerge }: { c: StudyChangeset; slug: string; pending: boolean; onMerge: () => void }) {
  const alone = c.only_with.length === 0
  const review = reviewWord(c.review)
  return (
    <tr className={cn('border-t border-border-subtle align-top', !alone && 'text-fg-secondary')}>
      <td className="px-2 py-1.5">
        <div className="font-medium whitespace-nowrap">
          #{c.k} {c.name}
        </div>
        <div className="text-xs text-fg-tertiary">
          {c.experiments.length} {c.experiments.length === 1 ? 'keep' : 'keeps'} · {c.files.length} {c.files.length === 1 ? 'file' : 'files'}
          {!alone && <span className="ml-1 text-warning">· only helps together with #{c.only_with.join(', #')}</span>}
        </div>
      </td>
      <td className={cn('px-2 py-1.5 text-right tabular-nums whitespace-nowrap', alone && 'text-success')}>{pct(c.delta_alone_pct)}</td>
      <td className="px-2 py-1.5 text-right tabular-nums whitespace-nowrap">{pct(c.delta_together_pct)}</td>
      <td className="hidden max-w-[260px] px-2 py-1.5 2xl:table-cell">
        <span className="line-clamp-2 font-mono text-xs text-fg-secondary" title={c.files.join('\n')}>
          {c.files.join(', ')}
        </span>
      </td>
      <td className="px-2 py-1.5">
        <span
          className={cn('rounded-full px-2 py-px text-[11px] font-semibold', review.tone === 'clean' ? 'bg-success-bg text-success' : review.tone === 'suspicious' ? 'bg-warning-bg text-warning' : 'bg-layer-2 text-fg-secondary')}
          title={c.review?.reason}
        >
          {review.word.toLowerCase()}
        </span>
      </td>
      <td className="px-2 py-1.5">
        <div className="flex flex-col items-end gap-1 2xl:flex-row 2xl:items-center 2xl:justify-end">
          <CopyButton text={prCommands(c, slug)} label="Copy push + PR commands" icon={<GitPullRequest />}>
            Open PR
          </CopyButton>
          <Button variant="outline" size="xs" title={`git merge --no-ff ${c.branch} into the repo's current branch (refused while your tree is dirty; nothing is pushed)`} disabled={pending} onClick={onMerge}>
            <GitMerge /> Merge into my branch
          </Button>
        </div>
      </td>
    </tr>
  )
}

/** The push + `gh pr create` lines for a changeset branch (the plugin never pushes). */
export function prCommands(c: StudyChangeset, slug: string): string {
  const title = `autoresearch(${slug}): ${c.name}`
  return `git push origin ${c.branch}\ngh pr create --head ${c.branch} --title ${JSON.stringify(title)} --body ${JSON.stringify(`Changeset #${c.k} of autoresearch study ${slug}: experiments #${c.experiments.join(', #')}; files ${c.files.join(', ')}.`)}`
}

/** Open PR puts the commands on the clipboard and says so. */
function CopyButton({ text, label, icon, children }: { text: string; label: string; icon: ReactNode; children: string }) {
  const [done, setDone] = useState(false)
  return (
    <Button
      variant="outline"
      size="xs"
      title={`${label}: ${text}`}
      onClick={() => {
        void navigator.clipboard?.writeText(text).then(() => {
          setDone(true)
          setTimeout(() => setDone(false), 1500)
        })
      }}
    >
      {done ? <Check /> : icon} {done ? 'Copied' : children}
      {!done && <Copy className="size-3 text-fg-tertiary" />}
    </Button>
  )
}

/** Discard study: removes the worktree and the study branches; one confirmation that names the slug (§11.5). */
export function DiscardStudy({ slug, pending, onConfirm }: { slug: string; pending: boolean; onConfirm: () => void }) {
  return (
    <AlertDialog>
      <AlertDialogTrigger asChild>
        <Button variant="ghost" size="xs" className="text-danger hover:text-danger" disabled={pending}>
          <Trash2 /> Discard study
        </Button>
      </AlertDialogTrigger>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Discard study {slug}?</AlertDialogTitle>
          <AlertDialogDescription>
            Removes its worktree and every <code className="font-mono">autoresearch/{slug}</code> branch. The ledger, the patches and the report stay. Your own checkout is not touched.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Keep</AlertDialogCancel>
          <AlertDialogAction onClick={onConfirm} className="bg-destructive text-primary-foreground hover:bg-destructive/90">
            Discard {slug}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
