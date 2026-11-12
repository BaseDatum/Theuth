import { withCapability } from '@/rpc/capabilities'
import { terminalRequirements } from '@/components/chat/requirements'
import { createFileRoute } from '@tanstack/react-router'
import { PoppedTerminal } from '@/components/chat/TerminalViewer'

/** One agent-started terminal, full window (`lib/popout.ts` kind `pty`). Under the `Popped` root when opened with `?window=`. */
export const Route = createFileRoute('/pty/$session/$job')({
  component: withCapability(PtyRoute, terminalRequirements),
})

function PtyRoute() {
  const { session, job } = Route.useParams()
  return <PoppedTerminal session={session} job={job} />
}
