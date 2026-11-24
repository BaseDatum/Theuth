import { withCapability } from '@/rpc/capabilities'
import { requirements } from '@/components/tasks/requirements'
import { createFileRoute } from '@tanstack/react-router'
import { SectionBand } from '@/components/nav/SectionBand'
import { Skeleton } from '@/components/ui/skeleton'
import { TaskGraph } from '@/components/tasks/graph/TaskGraph'
import { searchProject } from '@/components/tasks/graph/model'
import { useBoardProjects, useTasks } from '@/components/tasks/queries'

/** 214 D6: `/tasks/graph?project=` — the project's task graph; `project: ''` is every project, none yet the first one
 * (as on the board). A static segment, so it outranks `/tasks/$id`. */
export const Route = createFileRoute('/tasks/graph')({
  validateSearch: (s: Record<string, unknown>): { project?: string } => {
    const project = searchProject(s.project)
    return project === undefined ? {} : { project }
  },
  component: withCapability(Page, requirements),
})

function Page() {
  const search = Route.useSearch()
  const navigate = Route.useNavigate()
  const projects = useBoardProjects()
  // The first project is known once the lists load; until then a bare URL waits instead of drawing every project.
  const loading = useTasks().isPending
  const project = search.project ?? projects[0] ?? ''
  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <SectionBand group="agents" />
      {search.project === undefined && loading ? (
        <div className="p-6">
          <Skeleton className="h-8 w-64" />
        </div>
      ) : (
        <TaskGraph variant="page" project={project === '' ? null : project} onProject={(p) => void navigate({ search: { project: p ?? '' }, replace: true })} />
      )}
    </article>
  )
}
