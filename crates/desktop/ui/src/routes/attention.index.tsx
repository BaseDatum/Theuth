import { withCapability } from '@/rpc/capabilities'
import { panelRequirements as requirements } from '@/components/attention/requirements'
import { createFileRoute } from '@tanstack/react-router'
import { entryHead } from '@/components/attention/model'
import { useAttentionView } from '@/components/attention/Page'
import { Screen } from '@/components/attention/Screen'

/** `/attention` (101 D6, 207 §8.5): desktop shows the first card of the list as filtered and sorted, without a redirect; under `md` the pane is hidden and only the list shows. */
function Index() {
  const first = useAttentionView()?.entries[0]
  return first ? <Screen item={entryHead(first)} /> : null
}

export const Route = createFileRoute('/attention/')({ component: withCapability(Index, requirements) })
