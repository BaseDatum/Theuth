import { createFileRoute } from '@tanstack/react-router'
import { MeetingHeader } from '@/components/meeting/MeetingList'
import { StartForm } from '@/components/meeting/StartForm'

/** 53 §4.1: the start form (tray → *Start meeting…* lands here). */
export const Route = createFileRoute('/meetings/new')({ component: New })

function New() {
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <MeetingHeader title="Start meeting" />
      <div className="min-h-0 flex-1 overflow-y-auto">
        <StartForm />
      </div>
    </div>
  )
}
