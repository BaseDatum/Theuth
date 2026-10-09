// 156 D5: the meeting page's controls, in the 44 px row under the Library tabs (97b: controls go below the tabs) —
// not after the last transcript segment, not at the top of the transcript scroll. By state: Review has *Done
// reviewing* (a muted "Reviewed HH:MM" once `reviewed_at` is set) and *Summarize* (disabled until reviewed); Done
// has *Re-run post pipeline*; Failed has the strip's fix action; Post/Summarizing/live have none (the strip spins,
// pause/stop stay in `MeetingBar`). *Delete meeting…* sits in a trailing ⋯ menu in every ended state. The same calls
// and caches as before: `meeting_edit([{op:'Reviewed'}])` written into `meetingTranscript(id)`, `meeting_summarize`,
// `meeting_post_retry`, `deleteMeeting`'s confirm. `actionsFor` and `layout` are the pure parts (tested).

import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { LoaderCircle, MoreHorizontal } from 'lucide-react'
import type { ReactNode } from 'react'
import type { Lifecycle } from '@/components/meeting/ControlCenter'
import { deleteMeeting } from '@/components/meeting/MeetingList'
import { useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { closePopout, isPopout } from '@/lib/popout'
import { queryKeys, useApi } from '@/lib/rpc'
import type { MeetingState } from '@/rpc/bindings/MeetingState'

export type RowAction =
  | { kind: 'reviewed'; at: string }
  | { kind: 'done_reviewing' }
  | { kind: 'summarize'; enabled: boolean }
  | { kind: 'retry_summary' }
  | { kind: 'rerun' }

/** The row's actions for a meeting in `state`, left to right. `cycle` is `lifecycle()`'s strip (its fix action for Failed). */
export function actionsFor(state: MeetingState | undefined, reviewedAt: string | null | undefined, cycle: Lifecycle | null): RowAction[] {
  switch (state) {
    case 'Review':
      return [reviewedAt ? { kind: 'reviewed', at: reviewedAt } : { kind: 'done_reviewing' }, { kind: 'summarize', enabled: !!reviewedAt }]
    case 'Done':
      return [{ kind: 'rerun' }]
    case 'Failed':
      return cycle === null ? [] : [cycle.action === 'summarize' ? { kind: 'retry_summary' } : { kind: 'rerun' }]
    default:
      return []
  }
}

/** Delete is offered once the recording is over: every lifecycle state, and a start that failed. */
export const deletable = (state: MeetingState | undefined, cycle: Lifecycle | null): boolean => cycle !== null || state === 'Failed'

/**
 * D6: on a phone the row keeps one compact button — the step that is current (the first enabled one) — and the rest
 * overflow into the ⋯ menu. Desktop shows them all in the row.
 */
export function layout(actions: RowAction[], compact: boolean): { row: RowAction[]; menu: RowAction[] } {
  if (!compact) return { row: actions, menu: [] }
  const primary = actions.find((a) => a.kind !== 'reviewed' && !(a.kind === 'summarize' && !a.enabled)) ?? actions[0]
  return { row: primary ? [primary] : [], menu: actions.filter((a) => a !== primary) }
}

const labelOf = (a: RowAction): string =>
  a.kind === 'reviewed'
    ? `Reviewed ${new Date(a.at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}`
    : a.kind === 'done_reviewing'
      ? 'Done reviewing'
      : a.kind === 'summarize'
        ? 'Summarize'
        : a.kind === 'retry_summary'
          ? 'Retry summary'
          : 'Re-run post pipeline'

const RERUN_CONFIRM = 'Re-run the post pipeline? Review edits are replaced; the note goes back to before the summary.'

/** `children` sit between the actions and the ⋯ menu (the Briefs toggle, the Session link). */
export function MeetingActions({ id, state, cycle, compact, children }: { id: string; state: MeetingState | undefined; cycle: Lifecycle | null; compact: boolean; children?: ReactNode }) {
  const api = useApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  // The transcript the review pane holds (same key, so one fetch); only Review reads `reviewed_at`.
  const transcript = useQuery({ queryKey: queryKeys.meetingTranscript(id), queryFn: () => api.meeting_transcript(id), enabled: state === 'Review' })
  const reviewed = useAction(() => api.meeting_edit(id, [{ op: 'Reviewed' }]), { onSuccess: (t) => qc.setQueryData(queryKeys.meetingTranscript(id), t) })
  const summarize = useAction(() => api.meeting_summarize(id), { keys: [queryKeys.meeting(id)] })
  const rerun = useAction(() => api.meeting_post_retry(id), { keys: [queryKeys.meeting(id)] })
  const del = useAction(() => deleteMeeting(api, id), {
    keys: [queryKeys.meetings],
    onSuccess: (done) => done && (isPopout() ? closePopout() : void navigate({ to: '/meetings' })),
  })
  const busy = reviewed.isPending || summarize.isPending || rerun.isPending
  const run = (a: RowAction) => {
    if (a.kind === 'done_reviewing') reviewed.mutate()
    else if (a.kind === 'summarize' || a.kind === 'retry_summary') summarize.mutate()
    else if (a.kind === 'rerun' && confirm(RERUN_CONFIRM)) rerun.mutate()
  }
  const { row, menu } = layout(actionsFor(state, transcript.data?.reviewed_at, cycle), compact)
  const canDelete = deletable(state, cycle)
  return (
    <>
      {row.map((a) =>
        a.kind === 'reviewed' ? (
          <span key={a.kind} className="shrink-0 text-[12px] text-fg-tertiary">
            {labelOf(a)}
          </span>
        ) : (
          <Button
            key={a.kind}
            size={compact ? 'xs' : 'sm'}
            variant={a.kind === 'summarize' ? 'default' : 'outline'}
            disabled={busy || (a.kind === 'summarize' && !a.enabled)}
            onClick={() => run(a)}
          >
            {a.kind === 'summarize' && summarize.isPending && <LoaderCircle className="animate-spin" />}
            {labelOf(a)}
          </Button>
        ),
      )}
      {children}
      {(menu.length > 0 || canDelete) && (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="icon-sm" aria-label="Meeting options">
              <MoreHorizontal />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            {menu.map((a) => (
              <DropdownMenuItem key={a.kind} disabled={a.kind === 'reviewed' || busy || (a.kind === 'summarize' && !a.enabled)} onSelect={() => run(a)}>
                {labelOf(a)}
              </DropdownMenuItem>
            ))}
            {menu.length > 0 && canDelete && <DropdownMenuSeparator />}
            {canDelete && (
              <DropdownMenuItem variant="destructive" disabled={del.isPending} onSelect={() => del.mutate()}>
                Delete meeting{'\u2026'}
              </DropdownMenuItem>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
      )}
    </>
  )
}
