import { createFileRoute, useNavigate, useRouter } from '@tanstack/react-router'
import { PairScreen } from '@/components/link/PairScreen'
import { useEngines } from '@/link/engines'

/**
 * "Pair with a mesh…" and the relay-served `/link#1.…` landing while a node or mesh is already active (31 §7.5).
 * `?repair=<id>`: re-pair an existing entry — the new pairing keeps its name and place (LINK-MSG).
 */
export const Route = createFileRoute('/link')({
  validateSearch: (s: Record<string, unknown>): { repair?: string } => ({ ...(typeof s.repair === 'string' && { repair: s.repair }) }),
  component: Link,
})

function Link() {
  const navigate = useNavigate()
  const router = useRouter()
  const { repair } = Route.useSearch()
  const replace = useEngines((s) => s.engines.find((h) => h.id === repair))
  const home = () => void navigate({ to: '/' })
  return <PairScreen replace={replace} onPaired={home} onBack={() => (router.history.canGoBack() ? router.history.back() : home())} />
}
