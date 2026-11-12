import { withCapability } from '@/rpc/capabilities'
import { panelRequirements as requirements } from '@/components/routines/requirements'
import { createFileRoute, redirect } from '@tanstack/react-router'
import { RoutineDetail } from '@/components/routines/RoutineDetail'
import { type Section, isSection } from '@/components/routines/helpers'
import { chatSearch } from './routines.new'

/** 140 R1-D4: an `autoresearch-<slug>` schedule is a study; its detail is the Study view, never RoutineDetail. */
const STUDY_PREFIX = 'autoresearch-'

export const Route = createFileRoute('/routines/$name/$section')({
  validateSearch: chatSearch,
  beforeLoad: ({ params }) => {
    // Covers the HUD ring link and deep links alike (R-88).
    if (params.name.startsWith(STUDY_PREFIX)) throw redirect({ to: '/autoresearch/$slug', params: { slug: params.name.slice(STUDY_PREFIX.length) }, search: {} })
    if (!isSection(params.section)) throw redirect({ to: '/routines/$name/$section', params: { name: params.name, section: 'overview' } })
  },
  component: withCapability(Page, requirements),
})

function Page() {
  const { name, section } = Route.useParams()
  const { chat } = Route.useSearch()
  return <RoutineDetail name={name} section={section as Section} chat={chat} />
}
