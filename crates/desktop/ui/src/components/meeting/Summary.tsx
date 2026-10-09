// 04 §7: `summary.md` on `/meetings/$id` once `Done`, for a meeting without a note. 59 §7.4: a meeting with a note has
// no `summary.md` to show here — the note beside the transcript is the summary — so this renders nothing. 156 D5:
// *Re-run post pipeline* and *Delete* moved to the header row (`MeetingActions`).

import { useQuery } from '@tanstack/react-query'
import { Streamdown } from 'streamdown'
import { Empty, errorText } from '@/components/settings/shared'
import { queryKeys, useApi } from '@/lib/rpc'

export function Summary({ id, note }: { id: string; note: string | null }) {
  const api = useApi()
  const summary = useQuery({ queryKey: queryKeys.meetingSummary(id), queryFn: () => api.meeting_summary(id), enabled: note === null })
  if (note !== null) return null
  if (summary.isError) return <Empty>{errorText(summary.error)}</Empty>
  if (summary.data === undefined) return null
  return (
    <div className="py-2 text-sm">
      <Streamdown mode="static">{summary.data}</Streamdown>
    </div>
  )
}
