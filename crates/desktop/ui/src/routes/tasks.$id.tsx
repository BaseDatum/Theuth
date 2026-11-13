import { withCapability } from '@/rpc/capabilities'
import { panelRequirements as requirements } from '@/components/tasks/requirements'
import { createFileRoute } from '@tanstack/react-router'
import { TaskDetail } from '@/components/tasks/TaskDetail'

export const Route = createFileRoute('/tasks/$id')({ component: withCapability(Page, requirements) })

function Page() {
  const { id } = Route.useParams()
  return <TaskDetail id={id} />
}
