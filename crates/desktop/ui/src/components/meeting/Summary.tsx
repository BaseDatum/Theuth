// 04 §7: `/meetings/$id` once `Done` — the summary (59 §7.4: the meeting's note in the editor when it has one,
// else `summary.md` rendered), *Open chat* (the header's Session button is only shown while the meeting is
// live), *Re-run post pipeline*, *Delete*, and the transcript of record read-only below.

import { useQuery } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { Streamdown } from 'streamdown'
import { MeetingNote } from '@/components/meeting/Note'
import { SegmentList } from '@/components/meeting/Review'
import { Empty, errorText, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { queryKeys, useApi } from '@/lib/rpc'

export function Summary({ id, session, note }: { id: string; session: string | null; note: string | null }) {
  const api = useApi()
  const navigate = useNavigate()
  const summary = useQuery({ queryKey: queryKeys.meetingSummary(id), queryFn: () => api.meeting_summary(id), enabled: note === null })
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
          onClick={() => confirm('Re-run the post pipeline? Review edits are replaced; the note goes back to before the summary.') && retry.mutate()}
        >
          Re-run post pipeline
        </Button>
        <Button size="sm" variant="destructive" disabled={del.isPending} onClick={() => confirm('Delete this meeting and its recording?') && del.mutate()}>
          Delete
        </Button>
      </div>
      {note ? (
        <MeetingNote doc={note} className="min-h-[60vh]" />
      ) : (
        <>
          {summary.isError && <Empty>{errorText(summary.error)}</Empty>}
          {summary.data !== undefined && (
            <div className="text-sm">
              <Streamdown mode="static">{summary.data}</Streamdown>
            </div>
          )}
        </>
      )}
      <h3 className="border-t border-border-subtle pt-4 text-base font-semibold">Transcript</h3>
      {transcript.isError && <Empty>{errorText(transcript.error)}</Empty>}
      {transcript.data && <SegmentList id={id} transcript={transcript.data} />}
    </div>
  )
}
