// 04 §7: the summary block on `/meetings/$id` once `Done` — *Re-run post pipeline*, *Delete*, and `summary.md`
// rendered. 59 §7.4: a meeting with a note has no `summary.md` to show here; the note beside the transcript is
// the summary, so this block is the actions only. The transcript and the note render in `ControlCenter`.

import { useQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { Streamdown } from 'streamdown'
import { Empty, errorText, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { queryKeys, useApi } from '@/lib/rpc'

export function Summary({ id, note }: { id: string; note: string | null }) {
  const api = useApi()
  const navigate = useNavigate()
  const summary = useQuery({ queryKey: queryKeys.meetingSummary(id), queryFn: () => api.meeting_summary(id), enabled: note === null })
  const retry = useAction(() => api.meeting_post_retry(id), { keys: [queryKeys.meeting(id)] })
  const del = useAction(() => api.meeting_delete(id), {
    keys: [queryKeys.meetings],
    onSuccess: () => void navigate({ to: '/meetings' }),
  })
  return (
    <div className="flex flex-col gap-4 py-2">
      <div className="flex items-center gap-2">
        <Button
          size="sm"
          variant="outline"
          disabled={retry.isPending}
          onClick={() => confirm('Re-run the post pipeline? Review edits are replaced; the note goes back to before the summary.') && retry.mutate()}
        >
          Re-run post pipeline
        </Button>
        <Button size="sm" variant="destructive" disabled={del.isPending} onClick={() => confirm('Delete this meeting and its recording?') && del.mutate()}>
          Delete
        </Button>
      </div>
      {note === null && summary.isError && <Empty>{errorText(summary.error)}</Empty>}
      {note === null && summary.data !== undefined && (
        <div className="text-sm">
          <Streamdown mode="static">{summary.data}</Streamdown>
        </div>
      )}
    </div>
  )
}
