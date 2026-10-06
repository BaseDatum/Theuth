import { createFileRoute, redirect } from '@tanstack/react-router'
import { RoutineDetail } from '@/components/routines/RoutineDetail'
import { type Section, isSection } from '@/components/routines/helpers'
import { chatSearch } from './routines.new'

export const Route = createFileRoute('/routines/$name/$section')({
  validateSearch: chatSearch,
  beforeLoad: ({ params }) => {
    if (!isSection(params.section)) throw redirect({ to: '/routines/$name/$section', params: { name: params.name, section: 'overview' } })
  },
  component: Page,
})

function Page() {
  const { name, section } = Route.useParams()
  const { chat } = Route.useSearch()
  return <RoutineDetail name={name} section={section as Section} chat={chat} />
}
