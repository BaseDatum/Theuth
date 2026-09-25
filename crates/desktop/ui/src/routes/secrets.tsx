import { createFileRoute } from '@tanstack/react-router'
import { Secrets } from '@/components/library/Secrets'

/** `?session=<id>`: carried over from the chat it was opened from so a redirect from `/settings/secrets` keeps its search; unused here. */
const validateSearch = (s: Record<string, unknown>): { session?: string } => ({
  ...(typeof s.session === 'string' && { session: s.session }),
})

export const Route = createFileRoute('/secrets')({ validateSearch, component: Secrets })
