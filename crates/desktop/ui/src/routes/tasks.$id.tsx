import { createFileRoute } from '@tanstack/react-router'
import { TaskDetail } from '@/components/tasks/TaskDetail'

export const Route = createFileRoute('/tasks/$id')({ component: Page })

function Page() {
  const { id } = Route.useParams()
  return <TaskDetail id={id} />
}
