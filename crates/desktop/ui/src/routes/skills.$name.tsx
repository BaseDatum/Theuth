import { createFileRoute } from '@tanstack/react-router'
import { SkillsPage } from '@/components/library/Skills'

/** `?session=<id>`: the chat the page was opened from; scopes the per-chat rows. */
const validateSearch = (s: Record<string, unknown>): { session?: string } => ({
  ...(typeof s.session === 'string' && { session: s.session }),
})

export const Route = createFileRoute('/skills/$name')({ validateSearch, component: Page })

function Page() {
  const { name } = Route.useParams()
  const { session } = Route.useSearch()
  return <SkillsPage name={name} session={session ?? null} />
}
