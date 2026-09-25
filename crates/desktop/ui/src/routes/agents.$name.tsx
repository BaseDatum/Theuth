import { createFileRoute } from '@tanstack/react-router'
import { AgentsView } from '@/components/agents/AgentsView'

export const Route = createFileRoute('/agents/$name')({ component: Page })

function Page() {
  const { name } = Route.useParams()
  return <AgentsView name={name} />
}
