import { useQuery } from '@tanstack/react-query'
import type { CSSProperties } from 'react'
import { NoteView } from '@/components/notes/NotePage'
import { Empty, errorText } from '@/components/settings/shared'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'

/**
 * The meeting's note (59 §7.4; 156 D1): the same full note as `/notes/<id>` — title, action bar, "Ask theuth…",
 * snapshots, export, pop-out — embedded in the meeting page's right pane: beside the live transcript while
 * recording, the summary view once the summary turn appended its sections. The same document, so what the user
 * typed during the call is what the agent regrouped — hover attribution tells the two apart. `meeting` hides
 * Delete (the meeting owns the note) and routes the note's chat to the meeting's own session (D2).
 */
export function MeetingNote({ doc, meeting, className, style }: { doc: string; meeting: string; className?: string; style?: CSSProperties }) {
  return (
    <div className={cn('flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden rounded-xl border border-border-subtle', className)} style={style}>
      <NoteView id={doc} embedded meeting={meeting} />
    </div>
  )
}

/** `/meetings/$id?pane=note` popped: the full note page in its own window (the popped `Frame` draws the drag header, `NoteView` sets the window title), the meeting's chat in its panel. */
export function PoppedMeetingNote({ id }: { id: string }) {
  const api = useApi()
  const manifest = useQuery({ queryKey: queryKeys.meeting(id), queryFn: () => api.meeting_get(id) })
  if (manifest.isError) return <Empty>{errorText(manifest.error)}</Empty>
  if (manifest.data && manifest.data.note === null) return <Empty>This meeting has no note.</Empty>
  if (!manifest.data?.note) return null
  return <NoteView key={manifest.data.note} id={manifest.data.note} meeting={id} />
}
