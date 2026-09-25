import { createFileRoute, useNavigate, useRouter } from '@tanstack/react-router'
import { PairScreen } from '@/components/link/PairScreen'

/** "Pair with a mesh…" and the relay-served `/link#1.…` landing while a node or mesh is already active (31 §7.5). */
export const Route = createFileRoute('/link')({ component: Link })

function Link() {
  const navigate = useNavigate()
  const router = useRouter()
  const home = () => void navigate({ to: '/' })
  return <PairScreen onPaired={home} onBack={() => (router.history.canGoBack() ? router.history.back() : home())} />
}
