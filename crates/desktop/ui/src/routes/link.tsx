import { createFileRoute, useNavigate } from '@tanstack/react-router'
import { PairScreen } from '@/components/link/PairScreen'

/** "Add harness…" and the relay-served `/link#1.…` landing while a harness is already active (31 §7.5). */
export const Route = createFileRoute('/link')({ component: Link })

function Link() {
  const navigate = useNavigate()
  return <PairScreen onPaired={() => void navigate({ to: '/' })} />
}
