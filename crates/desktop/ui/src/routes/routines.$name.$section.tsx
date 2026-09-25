import { createFileRoute, redirect } from '@tanstack/react-router'
import { RoutineDetail } from '@/components/routines/RoutineDetail'
import { type Section, isSection } from '@/components/routines/helpers'

export const Route = createFileRoute('/routines/$name/$section')({
  beforeLoad: ({ params }) => {
    if (!isSection(params.section)) throw redirect({ to: '/routines/$name/$section', params: { name: params.name, section: 'overview' } })
  },
  component: Page,
})

function Page() {
  const { name, section } = Route.useParams()
  return <RoutineDetail name={name} section={section as Section} />
}
