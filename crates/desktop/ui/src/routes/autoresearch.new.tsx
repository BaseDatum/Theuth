import { withCapability } from '@/rpc/capabilities'
import { panelRequirements as requirements } from '@/components/autoresearch/requirements'
import { createFileRoute } from '@tanstack/react-router'
import { SetupPane } from '@/components/autoresearch/SetupPane'
import { chatSearch } from './routines.new'

/** `/autoresearch/new` (151 §4.7): the setup pane — the intro box without `?chat`, that setup session with `?chat=<id>`. The static `new` beats `$slug`. */
export const Route = createFileRoute('/autoresearch/new')({ validateSearch: chatSearch, component: withCapability(Page, requirements) })

function Page() {
  const { chat } = Route.useSearch()
  return <SetupPane chat={chat} />
}
