import { createFileRoute } from '@tanstack/react-router'
import { ControlCenter } from '@/components/meeting/ControlCenter'
import { PoppedMeetingNote } from '@/components/meeting/Note'
import { POPOUT } from '@/lib/popout'

/** 53 §4.3: the control center while recording; the manifest after. Popped with `?pane=note`: the meeting's note alone. */
export const Route = createFileRoute('/meetings/$id')({ component: Page })

function Page() {
  const { id } = Route.useParams()
  return POPOUT?.kind === 'meeting-note' ? <PoppedMeetingNote id={id} /> : <ControlCenter id={id} />
}
