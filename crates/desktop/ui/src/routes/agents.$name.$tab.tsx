import { createFileRoute, redirect } from '@tanstack/react-router'
import { AgentDetail, TABS, type Tab } from '@/components/agents/AgentDetail'

export const Route = createFileRoute('/agents/$name/$tab')({
  beforeLoad: ({ params }) => {
    if (!(TABS as readonly string[]).includes(params.tab))
      throw redirect({ to: '/agents/$name/$tab', params: { name: params.name, tab: 'overview' }, replace: true })
  },
  component: Page,
})

function Page() {
  const { name, tab } = Route.useParams()
  return <AgentDetail name={name} tab={tab as Tab} />
}
