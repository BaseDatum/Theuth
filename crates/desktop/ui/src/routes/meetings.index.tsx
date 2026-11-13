import { withCapability } from '@/rpc/capabilities'
import { requirements } from '@/components/meeting/requirements'
import { createFileRoute } from '@tanstack/react-router'
import { MeetingList } from '@/components/meeting/MeetingList'

/** 03 §3: recorded meetings. */
export const Route = createFileRoute('/meetings/')({ component: withCapability(MeetingList, requirements) })
