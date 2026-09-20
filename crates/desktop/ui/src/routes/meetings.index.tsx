import { createFileRoute } from '@tanstack/react-router'
import { MeetingList } from '@/components/meeting/MeetingList'

/** 03 §3: recorded meetings. */
export const Route = createFileRoute('/meetings/')({ component: MeetingList })
