import { withCapability } from '@/rpc/capabilities'
import { panelRequirements as requirements } from '@/components/attention/requirements'
import { createFileRoute } from '@tanstack/react-router'
import { Screen } from '@/components/attention/Screen'
import { useAttention } from '@/components/hud/queries'

/** `/attention` (101 D6): desktop shows the newest item in the pane without a redirect; under `md` the pane is hidden and only the rail shows. */
function Index() {
  const attention = useAttention()
  const first = attention.data?.[0]
  return first ? <Screen item={first} /> : null
}

export const Route = createFileRoute('/attention/')({ component: withCapability(Index, requirements) })
