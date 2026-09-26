import { createFileRoute } from '@tanstack/react-router'
import { useEffect } from 'react'
import { NewChat } from '@/components/chat/NewChatPane'

export const Route = createFileRoute('/new')({
  // `?project=<label>`: the sidebar's "New chat in <project>" preselects the picker (51 §3).
  validateSearch: (s: Record<string, unknown>): { project?: string } =>
    typeof s.project === 'string' && s.project !== '' ? { project: s.project } : {},
  component: New,
})

/** New chat: nothing exists until the first `send_input(null, …)` / `create_session` reply names the session. */
function New() {
  const { project: preset } = Route.useSearch()
  useEffect(() => {
    document.title = ''
  }, [])
  return <NewChat preset={preset} />
}
