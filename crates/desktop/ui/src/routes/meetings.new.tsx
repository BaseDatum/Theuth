import { withCapability } from '@/rpc/capabilities'
import { panelRequirements as requirements } from '@/components/meeting/requirements'
import { createFileRoute } from '@tanstack/react-router'
import { MeetingHeader, useCanRecord } from '@/components/meeting/MeetingList'
import { StartForm } from '@/components/meeting/StartForm'
import { Empty } from '@/components/settings/shared'

/** 53 §4.1: the start form (tray → *Start meeting…* lands here). Off the local Mac (phone, browser, relay) it only explains. */
export const Route = createFileRoute('/meetings/new')({ component: withCapability(New, requirements) })

function New() {
  const canRecord = useCanRecord()
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <MeetingHeader title="Start meeting" back />
      <div className="min-h-0 flex-1 overflow-y-auto">
        {canRecord ? (
          <StartForm />
        ) : (
          <div className="px-6">
            <Empty>Meetings are recorded from the Mac running the node. Start one there.</Empty>
          </div>
        )}
      </div>
    </div>
  )
}
