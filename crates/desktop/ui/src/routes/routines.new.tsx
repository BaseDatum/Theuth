import { createFileRoute } from '@tanstack/react-router'
import { RoutineDetail } from '@/components/routines/RoutineDetail'

/** `?chat=1` opens the Describe-it pane in its intro state; `?chat=<session>` that session (108 §6.3). */
export type ChatSearch = { chat?: string }
export const chatSearch = (s: Record<string, unknown>): ChatSearch => (typeof s.chat === 'string' && s.chat ? { chat: s.chat } : {})

export const Route = createFileRoute('/routines/new')({ validateSearch: chatSearch, component: Page })

function Page() {
  const { chat } = Route.useSearch()
  return <RoutineDetail name={null} section="overview" chat={chat} />
}
