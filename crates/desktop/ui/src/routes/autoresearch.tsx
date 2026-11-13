import { withCapability } from '@/rpc/capabilities'
import { requirements } from '@/components/autoresearch/requirements'
import { createFileRoute } from '@tanstack/react-router'
import { Explorer } from '@/components/autoresearch/Explorer'

/** `/autoresearch` (140): the Agents tab next to Routines — the rail of studies and the Study view outlet. `?q=` prefills the rail's filter. */
export const Route = createFileRoute('/autoresearch')({
  validateSearch: (s: Record<string, unknown>): { q?: string } => (typeof s.q === 'string' && s.q !== '' ? { q: s.q } : {}),
  component: withCapability(Explorer, requirements),
})
