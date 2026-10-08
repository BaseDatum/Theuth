import { Link } from '@tanstack/react-router'
import { ExternalLink, FileText, GitMerge, GitPullRequest, Trash2 } from 'lucide-react'
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle, AlertDialogTrigger } from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { Panel } from './SidePanel'
import { STOP_REASON_LABEL, pct } from './model'
import { useControl } from './queries'
import type { Changeset, StudyDetail } from './types'

/**
 * §11.5 the finished state: the headline, the changesets table (Δ alone / Δ together / files / review) with
 * Open PR · Merge into my branch per changeset, the report link, and Discard study (one confirmation of the label).
 */
export function Finished({ detail }: { detail: StudyDetail }) {
  const control = useControl(detail.info.slug)
  const sets = detail.changesets ?? []
  return (
    <Panel
      title="Finished"
      hint={detail.stop_reason ? STOP_REASON_LABEL[detail.stop_reason] : undefined}
      actions={
        <>
          {detail.report_doc && (
            <Button asChild variant="outline" size="xs">
              <Link to="/notes/$id" params={{ id: detail.report_doc }} search={{}}>
                <FileText /> Report
              </Link>
            </Button>
          )}
          <DiscardStudy slug={detail.info.slug} pending={control.isPending} onConfirm={() => control.mutate('Discard')} />
        </>
      }
      className="border-success-border/40"
    >
      {detail.headline && <p className="mb-3 text-[15px] font-semibold tabular-nums">{detail.headline}</p>}
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
                <Row key={c.k} c={c} pending={control.isPending} onPr={() => control.mutate({ OpenPr: { k: c.k } })} onMerge={() => control.mutate({ Merge: { k: c.k } })} />
              ))}
            </tbody>
          </table>
        </div>
      )}
      <p className="mt-2 text-xs text-fg-tertiary">
        Each changeset is a local branch <code className="font-mono">autoresearch/{detail.info.slug}/&lt;k&gt;-&lt;name&gt;</code>; nothing was pushed. Merge refuses while your checkout is dirty.
      </p>
    </Panel>
  )
}

function Row({ c, pending, onPr, onMerge }: { c: Changeset; pending: boolean; onPr: () => void; onMerge: () => void }) {
  return (
    <tr className={cn('border-t border-border-subtle align-top', !c.alone_clears && 'text-fg-secondary')}>
      <td className="px-2 py-1.5">
        <div className="font-medium whitespace-nowrap">
          #{c.k} {c.name}
        </div>
        <div className="text-xs text-fg-tertiary">
          {c.commits.length} {c.commits.length === 1 ? 'commit' : 'commits'} · {c.files.length} {c.files.length === 1 ? 'file' : 'files'}
          {!c.alone_clears && c.depends_on !== null && <span className="ml-1 text-warning">· only helps together with #{c.depends_on}</span>}
          {c.merged && <span className="ml-1 text-success">· merged</span>}
        </div>
      </td>
      <td className={cn('px-2 py-1.5 text-right tabular-nums whitespace-nowrap', c.alone_clears && 'text-success')}>{pct(c.delta_alone_pct)}</td>
      <td className="px-2 py-1.5 text-right tabular-nums whitespace-nowrap">{pct(c.delta_together_pct)}</td>
      <td className="hidden max-w-[260px] px-2 py-1.5 2xl:table-cell">
        <span className="line-clamp-2 font-mono text-xs text-fg-secondary" title={c.files.join('\n')}>
          {c.files.join(', ')}
        </span>
      </td>
      <td className="px-2 py-1.5">
        <span className={cn('rounded-full px-2 py-px text-[11px] font-semibold', c.review === 'clean' ? 'bg-success-bg text-success' : c.review === 'suspicious' ? 'bg-warning-bg text-warning' : 'bg-layer-2 text-fg-secondary')}>{c.review}</span>
      </td>
      <td className="px-2 py-1.5">
        <div className="flex flex-col items-end gap-1 2xl:flex-row 2xl:items-center 2xl:justify-end">
          {c.pr_url ? (
            <Button asChild variant="outline" size="xs">
              <a href={c.pr_url} target="_blank" rel="noreferrer">
                <ExternalLink /> PR
              </a>
            </Button>
          ) : (
            <Button variant="outline" size="xs" disabled={pending || c.merged} onClick={onPr} title="A pull request from the changeset branch; its body is the report's section">
              <GitPullRequest /> Open PR
            </Button>
          )}
          <Button variant="outline" size="xs" disabled={pending || c.merged} onClick={onMerge} title="git merge --no-ff into the repo's current branch; refused if your tree is dirty">
            <GitMerge /> Merge into my branch
          </Button>
        </div>
      </td>
    </tr>
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
          <AlertDialogDescription>Removes its worktree and every <code className="font-mono">autoresearch/{slug}</code> branch. The ledger, the patches and the report stay. Your own checkout is not touched.</AlertDialogDescription>
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
