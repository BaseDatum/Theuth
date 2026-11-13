import { withCapability } from '@/rpc/capabilities'
import { panelRequirements as requirements } from '@/components/routines/requirements'
import { createFileRoute } from '@tanstack/react-router'
import { RoutineDetail } from '@/components/routines/RoutineDetail'

/** `?chat=1` opens the Describe-it pane in its intro state; `?chat=<session>` that session (108 §6.3). */
export type ChatSearch = { chat?: string }
// The default search parser reads `?chat=1` as the number 1; a hand-typed URL must still open the intro.
export const chatSearch = (s: Record<string, unknown>): ChatSearch => {
  const c = typeof s.chat === 'number' ? String(s.chat) : s.chat
  return typeof c === 'string' && c ? { chat: c } : {}
}

export const Route = createFileRoute('/routines/new')({ validateSearch: chatSearch, component: withCapability(Page, requirements) })

function Page() {
  const { chat } = Route.useSearch()
  return <RoutineDetail name={null} section="overview" chat={chat} />
}
