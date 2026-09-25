import { createFileRoute, useNavigate, useRouter } from '@tanstack/react-router'
import { PairScreen } from '@/components/link/PairScreen'

/** "Add engine…" and the relay-served `/link#1.…` landing while an engine is already active (31 §7.5). */
export const Route = createFileRoute('/link')({ component: Link })

function Link() {
  const navigate = useNavigate()
  const router = useRouter()
  const home = () => void navigate({ to: '/' })
  return <PairScreen onPaired={home} onBack={() => (router.history.canGoBack() ? router.history.back() : home())} />
}
