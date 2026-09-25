import { createFileRoute } from '@tanstack/react-router'
import { Tools } from '@/components/library/Tools'

/** `?session=<id>`: the chat the page was opened from; scopes the per-chat rows. */
const validateSearch = (s: Record<string, unknown>): { session?: string } => ({
  ...(typeof s.session === 'string' && { session: s.session }),
})

export const Route = createFileRoute('/tools')({ validateSearch, component: Page })

function Page() {
  const { session } = Route.useSearch()
  return <Tools session={session ?? null} />
}
