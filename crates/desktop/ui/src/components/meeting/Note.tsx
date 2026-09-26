import { useQuery } from '@tanstack/react-query'
import { AppWindow } from 'lucide-react'
import { useEffect, useMemo } from 'react'
import { Live } from '@/components/editor/DocumentPage'
import { useDocument } from '@/components/editor/useDocument'
import { Empty, errorText } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { isPopout, openPopout } from '@/lib/popout'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'

/**
 * The meeting's note (59 §7.4) in BlockNote: beside the live transcript while recording, the summary view once
 * the summary turn appended its sections. The same document, so what the user typed during the call is what the
 * agent regrouped — hover attribution tells the two apart. With `meeting`, a header offers the note in its own
 * window (the same document over the same actor: two editors on one doc, like two devices).
 */
export function MeetingNote({ doc, meeting, className }: { doc: string; meeting?: string; className?: string }) {
  const target = useMemo(() => ({ doc }), [doc])
  const live = useDocument(target)
  return (
    <div className={cn('flex min-h-0 min-w-0 flex-1 flex-col rounded-xl border border-border-subtle', className)}>
      {meeting && !isPopout() && (
        <div className="flex h-8 shrink-0 items-center border-b border-border-subtle pr-1 pl-3 text-[12px] font-semibold text-fg-secondary">
          <span className="flex-1">Note</span>
          <Button variant="ghost" size="icon-sm" aria-label="Open note in new window" onClick={() => void openPopout({ kind: 'meeting-note', meeting })}>
            <AppWindow />
          </Button>
        </div>
      )}
      <Live {...live} dense />
    </div>
  )
}

/** `/meetings/$id?pane=note` popped: the note alone, full height, the meeting's title on the window. */
export function PoppedMeetingNote({ id }: { id: string }) {
  const api = useApi()
  const manifest = useQuery({ queryKey: queryKeys.meeting(id), queryFn: () => api.meeting_get(id) })
  const title = manifest.data?.title ?? (manifest.data ? `Meeting with ${manifest.data.process.name}` : '')
  useEffect(() => {
    document.title = title
  }, [title])
  if (manifest.isError) return <Empty>{errorText(manifest.error)}</Empty>
  if (manifest.data && manifest.data.note === null) return <Empty>This meeting has no note.</Empty>
  if (!manifest.data?.note) return null
  return <MeetingNote doc={manifest.data.note} className="rounded-none border-0" />
}
