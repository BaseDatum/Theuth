import { withCapability } from '@/rpc/capabilities'
import { requirements } from '@/components/projects/requirements'
import { createFileRoute } from '@tanstack/react-router'
import { ProjectDetail } from '@/components/projects/ProjectDetail'

/** `/projects/$name` (223): one project; `$name` is the label, URL-encoded. */
export const Route = createFileRoute('/projects/$name')({ component: withCapability(Page, requirements) })

function Page() {
  const { name } = Route.useParams()
  return <ProjectDetail key={name} name={name} />
}
