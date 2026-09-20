// 04 §7: `/meetings/$id` once `Done` — `summary.md` rendered, *Open chat* (the header's Session button is only shown
// while the meeting is live), *Re-run post pipeline*, *Delete*, and the transcript of record read-only below.

import { useQuery } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { Streamdown } from 'streamdown'
import { SegmentList } from '@/components/meeting/Review'
import { Empty, errorText, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { queryKeys, useApi } from '@/lib/rpc'

export function Summary({ id, session }: { id: string; session: string | null }) {
  const api = useApi()
  const navigate = useNavigate()
  const summary = useQuery({ queryKey: queryKeys.meetingSummary(id), queryFn: () => api.meeting_summary(id) })
  const transcript = useQuery({ queryKey: queryKeys.meetingTranscript(id), queryFn: () => api.meeting_transcript(id) })
  const retry = useAction(() => api.meeting_post_retry(id), { keys: [queryKeys.meeting(id)] })
  const del = useAction(() => api.meeting_delete(id), {
    keys: [queryKeys.meetings],
    onSuccess: () => void navigate({ to: '/meetings' }),
  })
  return (
    <div className="flex flex-col gap-4 py-2">
      <div className="flex items-center gap-2">
        {session && (
          <Button size="sm" asChild>
            <Link to="/c/$session" params={{ session }}>
              Open chat
            </Link>
          </Button>
        )}
        <Button
          size="sm"
          variant="outline"
          disabled={retry.isPending}
          onClick={() => confirm('Re-run the post pipeline? Review edits and the summary are replaced.') && retry.mutate()}
        >
          Re-run post pipeline
        </Button>
        <Button size="sm" variant="destructive" disabled={del.isPending} onClick={() => confirm('Delete this meeting and its recording?') && del.mutate()}>
          Delete
        </Button>
      </div>
      {summary.isError && <Empty>{errorText(summary.error)}</Empty>}
      {summary.data !== undefined && (
        <div className="text-sm">
          <Streamdown mode="static">{summary.data}</Streamdown>
        </div>
      )}
      <h3 className="border-t border-border-subtle pt-4 text-base font-semibold">Transcript</h3>
      {transcript.isError && <Empty>{errorText(transcript.error)}</Empty>}
      {transcript.data && <SegmentList id={id} transcript={transcript.data} />}
    </div>
  )
}
